//! Record/replay tests for the OpenSubtitles provider. No live provider is
//! touched: a [`RecordedFetcher`] serves fixed JSON/bytes, so the search
//! normalization + the two-step download run end to end against recorded data.

use cellarr_core::MediaType;
use cellarr_subtitles::{
    OpenSubtitles, OpenSubtitlesConfig, RecordedFetcher, SubtitleError, SubtitleMatch,
    SubtitleProvider, SubtitleQuery,
};

const BASE: &str = "https://os.test";

fn config(api_key: &str, user: Option<&str>, pass: Option<&str>) -> OpenSubtitlesConfig {
    OpenSubtitlesConfig {
        api_key: api_key.to_string(),
        username: user.map(str::to_string),
        password: pass.map(str::to_string),
        base_url: BASE.to_string(),
    }
}

const SEARCH_BODY: &str = r#"{
  "data": [
    {
      "id": "1", "type": "subtitle",
      "attributes": {
        "language": "en",
        "download_count": 12000,
        "hearing_impaired": false,
        "foreign_parts_only": false,
        "ratings": 8.5,
        "from_trusted": true,
        "release": "The.Matrix.1999.1080p.BluRay.x264",
        "files": [ { "file_id": 987654, "file_name": "The.Matrix.1999.srt" } ]
      }
    },
    {
      "id": "2", "type": "subtitle",
      "attributes": {
        "language": "en",
        "download_count": 20,
        "hearing_impaired": true,
        "foreign_parts_only": false,
        "ratings": 4.0,
        "from_trusted": false,
        "release": "The.Matrix.CAM",
        "files": [ { "file_id": 111, "file_name": "matrix.hi.srt" } ]
      }
    },
    {
      "id": "3", "type": "subtitle",
      "attributes": {
        "language": "en",
        "release": "no files here",
        "files": []
      }
    }
  ]
}"#;

#[tokio::test]
async fn search_normalizes_matches_and_drops_fileless_entries() {
    let fetcher = RecordedFetcher::new().with_body(&format!("{BASE}/api/v1/subtitles"), SEARCH_BODY);
    let os = OpenSubtitles::new(fetcher, config("key", None, None));

    let q = SubtitleQuery {
        media_type: Some(MediaType::Movie),
        imdb_id: Some("tt0133093".to_string()),
        languages: vec!["en".to_string()],
        ..Default::default()
    };
    let matches = os.search(&q).await.expect("search ok");

    // Two of the three entries have a downloadable file; the empty-files one is dropped.
    assert_eq!(matches.len(), 2);
    let top = &matches[0];
    assert_eq!(top.provider, "opensubtitles");
    assert_eq!(top.id, "987654");
    assert_eq!(top.language, "en");
    assert_eq!(top.release_name.as_deref(), Some("The.Matrix.1999.1080p.BluRay.x264"));
    assert!(!top.forced);
    assert!(!top.hearing_impaired);
    assert_eq!(top.format, "srt");
    // rating 8.5 -> 85, downloads 12000/500 -> 24, trusted +25 => 134.
    assert_eq!(top.score, 134);

    let hi = &matches[1];
    assert!(hi.hearing_impaired);
    // rating 4.0 -> 40, downloads 20/500 -> 0, untrusted +0 => 40.
    assert_eq!(hi.score, 40);
}

#[tokio::test]
async fn search_without_api_key_is_no_credential() {
    let os = OpenSubtitles::new(RecordedFetcher::new(), config("", None, None));
    let err = os.search(&SubtitleQuery::default()).await.unwrap_err();
    assert!(matches!(err, SubtitleError::NoCredential { .. }));
}

#[tokio::test]
async fn search_404_is_empty_not_error() {
    // An unregistered route 404s in the recorder → provider maps it to "no matches".
    let os = OpenSubtitles::new(RecordedFetcher::new(), config("key", None, None));
    let matches = os
        .search(&SubtitleQuery {
            imdb_id: Some("tt0000000".to_string()),
            languages: vec!["en".to_string()],
            ..Default::default()
        })
        .await
        .expect("404 is Ok(empty)");
    assert!(matches.is_empty());
}

#[tokio::test]
async fn download_logs_in_then_fetches_the_link_bytes() {
    let srt = "1\n00:00:01,000 --> 00:00:02,000\nHello\n";
    let fetcher = RecordedFetcher::new()
        .with_body(&format!("{BASE}/api/v1/login"), r#"{"token":"tok-123"}"#)
        .with_body(
            &format!("{BASE}/api/v1/download"),
            r#"{"link":"https://dl.os.test/x/the.matrix.srt","requests":1,"remaining":99}"#,
        )
        .with_body("https://dl.os.test/x/the.matrix.srt", srt);
    let os = OpenSubtitles::new(fetcher, config("key", Some("u"), Some("p")));

    let m = SubtitleMatch {
        provider: "opensubtitles",
        id: "987654".to_string(),
        language: "en".to_string(),
        release_name: None,
        forced: false,
        hearing_impaired: false,
        score: 100,
        format: "srt".to_string(),
    };
    let bytes = os.download(&m).await.expect("download ok");
    assert_eq!(String::from_utf8(bytes).unwrap(), srt);
}

#[tokio::test]
async fn download_without_login_credentials_is_no_credential() {
    let fetcher = RecordedFetcher::new()
        .with_body(&format!("{BASE}/api/v1/download"), r#"{"link":"x"}"#);
    // api key present, but no username/password → cannot mint the download token.
    let os = OpenSubtitles::new(fetcher, config("key", None, None));
    let m = SubtitleMatch {
        provider: "opensubtitles",
        id: "1".to_string(),
        language: "en".to_string(),
        release_name: None,
        forced: false,
        hearing_impaired: false,
        score: 0,
        format: "srt".to_string(),
    };
    let err = os.download(&m).await.unwrap_err();
    assert!(matches!(err, SubtitleError::NoCredential { .. }));
}
