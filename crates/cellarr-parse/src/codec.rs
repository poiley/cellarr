//! Video codec extraction (x264/x265/HEVC/AVC/AV1/XviD/DivX/MPEG-2).

use std::sync::LazyLock;

use cellarr_core::parsed::{Confidence, ParsedField, ParsedRelease, VideoCodec};
use regex::Regex;

static CODEC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        \b(
            x265 | h\.?265 | hevc |
            x264 | h\.?264 | avc |
            av1 |
            xvid | divx | mpeg-?2 | vc-?1 | wmv
        )\b",
    )
    .unwrap()
});

/// Extract the video codec.
pub fn extract(input: &str, out: &mut ParsedRelease) {
    // Match on the raw input so dotted spellings like `H.264` survive (the
    // separator-normalizer would turn the `.` into a space and break them).
    let Some(m) = CODEC.find(input) else {
        return;
    };
    let tok = m.as_str().to_ascii_lowercase().replace(['.', '-'], "");
    let codec = match tok.as_str() {
        "x265" | "h265" | "hevc" => VideoCodec::X265,
        "x264" | "h264" | "avc" => VideoCodec::X264,
        "av1" => VideoCodec::Av1,
        _ => VideoCodec::Other, // XviD, DivX, MPEG-2, VC-1, WMV
    };
    out.codec = Some(codec);
    out.set_confidence(ParsedField::Codec, Confidence::new(0.95));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codec(s: &str) -> Option<VideoCodec> {
        let mut p = ParsedRelease::new(s);
        extract(s, &mut p);
        p.codec
    }

    #[test]
    fn h264_family() {
        assert_eq!(codec("Show.S01E01.1080p.x264-GRP"), Some(VideoCodec::X264));
        assert_eq!(codec("Show.S01E01.1080p.H.264-GRP"), Some(VideoCodec::X264));
        assert_eq!(codec("Show.S01E01.1080p.AVC-GRP"), Some(VideoCodec::X264));
    }

    #[test]
    fn h265_family() {
        assert_eq!(codec("Show.S01E01.2160p.x265-GRP"), Some(VideoCodec::X265));
        assert_eq!(codec("Show.S01E01.2160p.HEVC-GRP"), Some(VideoCodec::X265));
    }

    #[test]
    fn legacy_is_other() {
        assert_eq!(codec("Movie.2001.DVDRip.XviD-GRP"), Some(VideoCodec::Other));
    }
}
