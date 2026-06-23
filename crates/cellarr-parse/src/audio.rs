//! Audio format extraction.
//!
//! Audio is free-form in `ParsedRelease` (a `Vec<String>`) because a release may
//! advertise a codec plus a channel layout plus an object-audio flag
//! (e.g. `TrueHD Atmos 7.1`). This phase collects the recognised audio tokens in
//! the order they appear and normalises their spelling.

use std::sync::LazyLock;

use cellarr_core::parsed::{Confidence, ParsedField, ParsedRelease};
use regex::Regex;

/// `(pattern, canonical)` — each recognised audio token and its canonical form.
const TOKENS: &[(&str, &str)] = &[
    (r"(?i)\btruehd\b", "TrueHD"),
    (r"(?i)\batmos\b", "Atmos"),
    (r"(?i)\bdts[\s._-]?x\b", "DTS-X"),
    (r"(?i)\bdts[\s._-]?hd([\s._-]?ma)?\b", "DTS-HD MA"),
    (r"(?i)\bdts\b", "DTS"),
    (r"(?i)\b(e[\s._-]?ac[\s._-]?3|ddp\d?|dd\+|eac3)", "EAC3"),
    (
        r"(?i)\b(ac[\s._-]?3|dd5\.?1|dd2\.?0|dolby[\s._-]?digital)",
        "AC3",
    ),
    (r"(?i)\baac(?:\b|\d)", "AAC"),
    (r"(?i)\b(flac)\b", "FLAC"),
    (r"(?i)\b(opus)\b", "Opus"),
    (r"(?i)\b(mp3)\b", "MP3"),
    (r"(?i)\b(lpcm|pcm)\b", "PCM"),
];

static REGEXES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    TOKENS
        .iter()
        .map(|(p, c)| (Regex::new(p).unwrap(), *c))
        .collect()
});

// Channel layouts like `7.1`, `5.1`, `2.0` are appended when present.
static CHANNELS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([257])\.[01]\b").unwrap());

/// Extract audio descriptors, in appearance order, de-duplicated.
pub fn extract(input: &str, out: &mut ParsedRelease) {
    // Match on the raw input so dotted spellings (`5.1`, `AAC2.0`, `DD5.1`)
    // survive — the separator-normalizer would collapse the `.` into a space.
    // The audio patterns tolerate `.`/`-`/space between sub-tokens explicitly.
    // Collect (position, canonical) so the output preserves left-to-right order.
    let mut found: Vec<(usize, String)> = Vec::new();
    for (re, canon) in REGEXES.iter() {
        if let Some(m) = re.find(input) {
            found.push((m.start(), (*canon).to_owned()));
        }
    }
    if let Some(m) = CHANNELS.find(input) {
        found.push((m.start(), m.as_str().to_owned()));
    }

    if found.is_empty() {
        return;
    }
    found.sort_by_key(|(pos, _)| *pos);

    let mut seen = std::collections::BTreeSet::new();
    for (_, canon) in found {
        if seen.insert(canon.clone()) {
            out.audio.push(canon);
        }
    }
    out.set_confidence(ParsedField::Audio, Confidence::new(0.85));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio(s: &str) -> Vec<String> {
        let mut p = ParsedRelease::new(s);
        extract(s, &mut p);
        p.audio
    }

    #[test]
    fn truehd_atmos() {
        let a = audio("Movie.2019.2160p.BluRay.TrueHD.Atmos.7.1-GRP");
        assert!(a.contains(&"TrueHD".to_string()));
        assert!(a.contains(&"Atmos".to_string()));
        assert!(a.contains(&"7.1".to_string()));
    }

    #[test]
    fn dts_hd() {
        let a = audio("Movie.2019.1080p.BluRay.DTS-HD.MA.5.1-GRP");
        assert!(a.contains(&"DTS-HD MA".to_string()));
    }

    #[test]
    fn aac_simple() {
        assert_eq!(audio("Show.S01E01.720p.WEB-DL.AAC2.0-GRP")[0], "AAC");
    }

    #[test]
    fn none_when_absent() {
        assert!(audio("Show.S01E01.720p.HDTV.x264-GRP").is_empty());
    }
}
