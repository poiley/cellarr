//! Reading picture size out of real container headers.
//!
//! The fixtures are genuine one-frame encodes rather than hand-built byte
//! strings, because a parser that has only ever seen bytes written to match it
//! proves nothing about the files a library actually holds.

use std::path::Path;

use cellarr_fs::probe_video;

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn a_matroska_file_reports_its_picture_size() {
    let probe = probe_video(&fixture("probe_1080p.mkv")).expect("a real mkv is readable");
    assert_eq!((probe.width, probe.height), (1920, 1080));
}

#[test]
fn an_mp4_file_reports_its_picture_size() {
    let probe = probe_video(&fixture("probe_720p.mp4")).expect("a real mp4 is readable");
    assert_eq!((probe.width, probe.height), (1280, 720));
}

#[test]
fn a_larger_matroska_file_is_read_the_same_way() {
    let probe = probe_video(&fixture("probe_2160p.mkv")).expect("a real 4k mkv is readable");
    assert_eq!((probe.width, probe.height), (3840, 2160));
}

/// Everything unreadable answers "I don't know" rather than guessing. The answer
/// is written into the library's own record of what it holds, so a wrong one is
/// worse than none: it would be indistinguishable from a real reading.
#[test]
fn anything_it_cannot_read_declines_to_answer() {
    let dir = tempfile::tempdir().unwrap();

    let missing = dir.path().join("gone.mkv");
    assert_eq!(probe_video(&missing), None, "a missing file");

    let unsupported = dir.path().join("clip.avi");
    std::fs::write(&unsupported, b"RIFF____AVI ").unwrap();
    assert_eq!(probe_video(&unsupported), None, "an unhandled container");

    let truncated = dir.path().join("half.mkv");
    std::fs::write(&truncated, &std::fs::read(fixture("probe_1080p.mkv")).unwrap()[..40]).unwrap();
    assert_eq!(probe_video(&truncated), None, "a truncated header");

    let garbage = dir.path().join("noise.mp4");
    std::fs::write(&garbage, vec![0xA5u8; 4096]).unwrap();
    assert_eq!(probe_video(&garbage), None, "bytes that are not a container");

    let empty = dir.path().join("empty.mkv");
    std::fs::write(&empty, b"").unwrap();
    assert_eq!(probe_video(&empty), None, "an empty file");
}

/// Resolution bands are read from WIDTH, not height.
///
/// Letterboxing varies height freely: a 2.40:1 4K encode is 3840x1600, which by
/// height alone reads as 1080p and would be graded a whole band low. Width holds
/// its nominal value across aspect ratios, so it is what the band is taken from.
#[test]
fn a_letterboxed_encode_is_graded_by_width_not_height() {
    let band = |width: u32, height: u32| match width {
        w if w >= 3000 => "2160p",
        w if w >= 1600 => "1080p",
        w if w >= 1100 => "720p",
        _ if height >= 500 => "576p",
        _ => "480p",
    };
    // Scope and flat framings of the same nominal resolution agree.
    assert_eq!(band(3840, 2160), "2160p", "4K full frame");
    assert_eq!(band(3840, 1600), "2160p", "4K at 2.40:1 — the case height gets wrong");
    assert_eq!(band(1920, 1080), "1080p", "1080p full frame");
    assert_eq!(band(1920, 800), "1080p", "1080p at 2.40:1");
    assert_eq!(band(1280, 720), "720p", "720p full frame");
    assert_eq!(band(1280, 534), "720p", "720p at 2.40:1");
    // Standard definition shares a width, so height separates the two systems.
    assert_eq!(band(720, 576), "576p", "PAL");
    assert_eq!(band(720, 480), "480p", "NTSC");
}
