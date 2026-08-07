//! Reading a video file's own dimensions out of its container header.
//!
//! A library scanned in place has no release title to parse, so its quality can
//! only come from the file itself. The container header states the picture size;
//! that is enough to place a file in a resolution band, and it is the only part
//! of the quality vocabulary a file can actually attest to.
//!
//! **What this cannot tell you.** Nothing in a container records where the video
//! came from — a Blu-ray rip, a web download and a broadcast capture at the same
//! resolution are byte-for-byte indistinguishable at this level. Callers deciding
//! a quality from a probe are choosing a source, not reading one.
//!
//! Parsing is deliberately shallow and fails closed: anything unexpected returns
//! `None` so the caller leaves the file as it found it. A wrong answer here would
//! be written into the library's own record of what it holds, so declining to
//! answer is always the better failure.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The picture size a container header reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoProbe {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// How far into a file to keep looking for the header before giving up.
///
/// Headers sit near one end or the other; a file that has not declared its
/// tracks within this much of walking is malformed or something this does not
/// understand, and either way the answer is "do not guess".
const MAX_WALK_BYTES: u64 = 256 * 1024 * 1024;

/// Read `path`'s picture size from its container header.
///
/// Returns `None` for an unsupported container, an unreadable file, or anything
/// the parse does not recognize.
#[must_use]
pub fn probe_video(path: &Path) -> Option<VideoProbe> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)?;
    let mut file = File::open(path).ok()?;
    let probe = match ext.as_str() {
        "mkv" | "webm" => matroska_dimensions(&mut file),
        "mp4" | "m4v" | "mov" => isobmff_dimensions(&mut file),
        // AVI/OGM and friends are not read here. They are overwhelmingly SD-era
        // content whose resolution band is not in doubt, and each would need its
        // own parser to answer a question worth little.
        _ => None,
    }?;
    // A zero dimension is the container declining to say, not a 0-pixel video.
    (probe.width > 0 && probe.height > 0).then_some(probe)
}

// --- Matroska / WebM ---------------------------------------------------------

const EBML_SEGMENT: u64 = 0x1853_8067;
const EBML_TRACKS: u64 = 0x1654_AE6B;
const EBML_TRACK_ENTRY: u64 = 0xAE;
const EBML_VIDEO: u64 = 0xE0;
const EBML_PIXEL_WIDTH: u64 = 0xB0;
const EBML_PIXEL_HEIGHT: u64 = 0xBA;

/// Read an EBML variable-length integer.
///
/// Returns the value and how many bytes it occupied. `keep_marker` distinguishes
/// the two uses: an element ID is identified by its full encoding including the
/// length marker, while a size strips the marker to get the number.
fn read_vint(file: &mut File, keep_marker: bool) -> Option<(u64, u32)> {
    let mut first = [0u8; 1];
    file.read_exact(&mut first).ok()?;
    let byte = first[0];
    if byte == 0 {
        return None;
    }
    let length = byte.leading_zeros() + 1;
    if length > 8 {
        return None;
    }
    let mut value = if keep_marker {
        u64::from(byte)
    } else {
        // Widened before shifting: an 8-byte vint shifts the mask out entirely,
        // which overflows an 8-bit shift. The first byte then contributes no
        // value bits, which a zero mask expresses correctly.
        let mask = u16::from(0xFFu8) >> length;
        u64::from(byte & u8::try_from(mask).unwrap_or(0))
    };
    for _ in 1..length {
        let mut next = [0u8; 1];
        file.read_exact(&mut next).ok()?;
        value = (value << 8) | u64::from(next[0]);
    }
    Some((value, length))
}

/// Read a big-endian unsigned integer of `len` bytes, as EBML stores them.
fn read_uint(file: &mut File, len: u64) -> Option<u64> {
    if len == 0 || len > 8 {
        return None;
    }
    let mut buf = [0u8; 8];
    let start = 8 - usize::try_from(len).ok()?;
    file.read_exact(&mut buf[start..]).ok()?;
    Some(u64::from_be_bytes(buf))
}

/// Walk the children of a master element, descending into the ones on the path
/// to the video track and stepping over everything else.
///
/// Stepping over is what keeps this cheap: a Matroska file is almost entirely
/// clusters of frames, and this seeks past them rather than reading them.
fn matroska_dimensions(file: &mut File) -> Option<VideoProbe> {
    let end = file.metadata().ok()?.len();
    // Skip the EBML header element to land on the Segment.
    find_in_master(file, 0, end.min(MAX_WALK_BYTES), 0)
}

/// Depth-limited search for the Video element's dimensions.
fn find_in_master(file: &mut File, start: u64, end: u64, depth: u32) -> Option<VideoProbe> {
    if depth > 6 {
        return None;
    }
    let mut pos = start;
    let mut width = None;
    let mut height = None;
    while pos < end {
        file.seek(SeekFrom::Start(pos)).ok()?;
        let (id, id_len) = read_vint(file, true)?;
        let (size, size_len) = read_vint(file, false)?;
        let header = u64::from(id_len) + u64::from(size_len);
        let body = pos.saturating_add(header);
        // An unknown-length element (all size bits set) is a live-muxed stream;
        // treat its body as running to the end of what we will walk.
        let body_end = if size == u64::MAX >> (64 - 7 * u64::from(size_len)) {
            end
        } else {
            body.saturating_add(size).min(end)
        };

        match id {
            EBML_SEGMENT | EBML_TRACKS | EBML_TRACK_ENTRY | EBML_VIDEO => {
                if id == EBML_VIDEO {
                    // Read this Video element's own dimension leaves.
                    let mut inner = body;
                    while inner < body_end {
                        file.seek(SeekFrom::Start(inner)).ok()?;
                        let (leaf_id, leaf_id_len) = read_vint(file, true)?;
                        let (leaf_size, leaf_size_len) = read_vint(file, false)?;
                        let leaf_body =
                            inner.saturating_add(u64::from(leaf_id_len) + u64::from(leaf_size_len));
                        if leaf_id == EBML_PIXEL_WIDTH {
                            file.seek(SeekFrom::Start(leaf_body)).ok()?;
                            width = read_uint(file, leaf_size).and_then(|v| u32::try_from(v).ok());
                        } else if leaf_id == EBML_PIXEL_HEIGHT {
                            file.seek(SeekFrom::Start(leaf_body)).ok()?;
                            height = read_uint(file, leaf_size).and_then(|v| u32::try_from(v).ok());
                        }
                        if let (Some(w), Some(h)) = (width, height) {
                            return Some(VideoProbe {
                                width: w,
                                height: h,
                            });
                        }
                        inner = leaf_body.checked_add(leaf_size)?;
                    }
                } else if let Some(found) = find_in_master(file, body, body_end, depth + 1) {
                    return Some(found);
                }
            }
            _ => {}
        }
        pos = body_end.max(body);
        if body_end <= body && size == 0 {
            pos = body;
        }
        if pos <= start && depth > 0 {
            return None; // no forward progress; malformed
        }
    }
    None
}

// --- ISO base media (MP4/MOV) ------------------------------------------------

/// Read the picture size from the track header boxes.
///
/// `tkhd` carries the track's display size as its last two 16.16 fixed-point
/// fields. Every track has one, and the non-video tracks report zero, so the
/// largest reported size is the picture.
fn isobmff_dimensions(file: &mut File) -> Option<VideoProbe> {
    let end = file.metadata().ok()?.len();
    let mut best: Option<VideoProbe> = None;
    walk_boxes(file, 0, end.min(MAX_WALK_BYTES), 0, &mut best);
    best
}

fn walk_boxes(file: &mut File, start: u64, end: u64, depth: u32, best: &mut Option<VideoProbe>) {
    if depth > 6 {
        return;
    }
    let mut pos = start;
    while pos.saturating_add(8) <= end {
        if file.seek(SeekFrom::Start(pos)).is_err() {
            return;
        }
        let mut header = [0u8; 8];
        if file.read_exact(&mut header).is_err() {
            return;
        }
        let size32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let kind = [header[4], header[5], header[6], header[7]];
        let (size, header_len) = match size32 {
            // 0 means "to end of file"; 1 means a 64-bit size follows.
            0 => (end - pos, 8u64),
            1 => {
                let mut ext = [0u8; 8];
                if file.read_exact(&mut ext).is_err() {
                    return;
                }
                (u64::from_be_bytes(ext), 16u64)
            }
            n => (u64::from(n), 8u64),
        };
        if size < header_len {
            return;
        }
        let body = pos.saturating_add(header_len);
        let body_end = pos.saturating_add(size).min(end);
        match &kind {
            // Containers on the path to the track headers.
            b"moov" | b"trak" => walk_boxes(file, body, body_end, depth + 1, best),
            b"tkhd" => {
                if let Some(probe) = read_tkhd(file, body, body_end) {
                    if best.is_none_or(|b| u64::from(probe.width) * u64::from(probe.height)
                        > u64::from(b.width) * u64::from(b.height))
                    {
                        *best = Some(probe);
                    }
                }
            }
            _ => {}
        }
        if body_end <= pos {
            return;
        }
        pos = body_end;
    }
}

/// The display size is the final eight bytes of a `tkhd`: two 16.16 fixed-point
/// numbers. Reading them from the end avoids caring which version's fields
/// precede them.
fn read_tkhd(file: &mut File, body: u64, body_end: u64) -> Option<VideoProbe> {
    if body_end < body.saturating_add(8) {
        return None;
    }
    file.seek(SeekFrom::Start(body_end - 8)).ok()?;
    let mut buf = [0u8; 8];
    file.read_exact(&mut buf).ok()?;
    let width = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) >> 16;
    let height = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]) >> 16;
    Some(VideoProbe { width, height })
}
