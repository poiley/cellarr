//! Record/replay tests for the Cardigann engine skeleton.
//!
//! The definition and the search-results page are synthetic fixtures (see
//! `tests/fixtures/NOTES.md`). We parse the definition and run its CSS
//! `rows`/`fields` selectors over the recorded HTML, asserting normalization into
//! `Release` — no tracker is contacted.

use cellarr_core::{IndexerId, Protocol};
use cellarr_indexers::cardigann::CardigannIndexer;
use cellarr_indexers::Definition;

const DEFINITION: &str = include_str!("fixtures/cardigann_mytracker.yml");
const RESULTS_HTML: &str = include_str!("fixtures/cardigann_mytracker.html");

#[test]
fn parses_definition_metadata_and_caps() {
    let def = Definition::from_yaml(DEFINITION).expect("parse definition");
    assert_eq!(def.id, "mytracker");
    assert_eq!(def.name, "My Example Tracker");
    assert!(def.has_mode("search"));
    assert!(def.has_mode("tv-search"));
    assert!(def.has_mode("movie-search"));
    assert_eq!(def.caps.categorymappings.len(), 3);
    // Category mappings are read from the definition, never hardcoded.
    let hd = def
        .caps
        .categorymappings
        .iter()
        .find(|m| m.id == "10")
        .expect("category 10");
    assert_eq!(hd.cat, "5040");
}

#[test]
fn extracts_releases_from_recorded_html() {
    let def = Definition::from_yaml(DEFINITION).expect("parse definition");
    let id = IndexerId::new();
    let engine = CardigannIndexer::new(id, def);

    let releases = engine.extract(RESULTS_HTML).expect("extract");

    // The non-torrent advertisement row must be ignored by the row selector.
    assert_eq!(releases.len(), 2, "only torrent rows are extracted");

    let first = &releases[0];
    assert_eq!(first.indexer_id, id);
    assert_eq!(first.protocol, Protocol::Torrent);
    assert_eq!(first.title, "Example.Show.S01E01.1080p.WEB-DL.H264-GROUP");
    // `download` field reads the href attribute of a.download.
    assert_eq!(first.download_url, "/download.php?id=501&authkey=xyz");
    // `details` maps to guid.
    assert_eq!(first.guid.as_deref(), Some("/details.php?id=501"));
    // "1.5 GB" parsed to bytes.
    assert_eq!(first.size, Some((1.5 * 1024.0 * 1024.0 * 1024.0) as u64));
    assert_eq!(first.seeders, Some(88));

    let second = &releases[1];
    assert_eq!(second.title, "Example.Show.S01E02.720p.HDTV.x264-OTHER");
    assert_eq!(second.size, Some(700 * 1024 * 1024));
    assert_eq!(second.seeders, Some(12));
}
