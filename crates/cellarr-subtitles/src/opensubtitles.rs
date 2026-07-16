//! The OpenSubtitles.com provider (REST API v1).
//!
//! Search needs only the `Api-Key` header and is anonymous; **download** needs a
//! logged-in Bearer token (username/password → `/login`), which is minted lazily
//! on the first download and cached. The two-step download mirrors the API:
//! `POST /download {file_id}` returns a one-shot `link`, which we then GET for the
//! raw subtitle bytes. All HTTP goes through the [`Fetcher`] seam so the whole
//! path — search, login, download, link-fetch — is exercised offline in tests.

use async_trait::async_trait;
use cellarr_core::MediaType;
use serde_json::Value;

use crate::error::SubtitleError;
use crate::http::Fetcher;
use crate::provider::{SubtitleMatch, SubtitleProvider, SubtitleQuery};

const SOURCE: &str = "opensubtitles";
const DEFAULT_BASE_URL: &str = "https://api.opensubtitles.com";

/// Configuration for the OpenSubtitles provider. `api_key` is required for any
/// call; `username`/`password` are required only to download (the API gates
/// downloads behind a per-user quota).
#[derive(Clone)]
pub struct OpenSubtitlesConfig {
    /// The OpenSubtitles API key (bring-your-own).
    pub api_key: String,
    /// Account username, needed for download.
    pub username: Option<String>,
    /// Account password, needed for download.
    pub password: Option<String>,
    /// Base URL; overridable for tests. Defaults to `https://api.opensubtitles.com`.
    pub base_url: String,
}

impl OpenSubtitlesConfig {
    /// Build a config with the default base URL.
    #[must_use]
    pub fn new(api_key: String, username: Option<String>, password: Option<String>) -> Self {
        Self {
            api_key,
            username,
            password,
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }
}

/// The OpenSubtitles provider, generic over the [`Fetcher`] so tests bind a
/// recorded transport and the daemon binds `reqwest`.
pub struct OpenSubtitles<F: Fetcher> {
    fetcher: F,
    config: OpenSubtitlesConfig,
    /// The Bearer token, minted lazily on first download and cached.
    token: tokio::sync::Mutex<Option<String>>,
}

impl<F: Fetcher> OpenSubtitles<F> {
    /// Build a provider from a fetcher and config.
    #[must_use]
    pub fn new(fetcher: F, config: OpenSubtitlesConfig) -> Self {
        Self {
            fetcher,
            config,
            token: tokio::sync::Mutex::new(None),
        }
    }

    fn api_headers(&self) -> Vec<(&str, &str)> {
        vec![
            ("Api-Key", self.config.api_key.as_str()),
            ("Content-Type", "application/json"),
            ("Accept", "application/json"),
        ]
    }

    /// The current Bearer token, logging in once if needed. Errors when no
    /// username/password is configured (download is impossible without them).
    async fn bearer(&self) -> Result<String, SubtitleError> {
        let mut guard = self.token.lock().await;
        if let Some(t) = guard.as_ref() {
            return Ok(t.clone());
        }
        let (Some(user), Some(pass)) = (&self.config.username, &self.config.password) else {
            return Err(SubtitleError::NoCredential { src: SOURCE });
        };
        let url = format!("{}/api/v1/login", self.config.base_url);
        let body = serde_json::json!({ "username": user, "password": pass });
        let resp = self.fetcher.post_json(&url, &body, &self.api_headers()).await?;
        if !resp.is_success() {
            return Err(SubtitleError::Http {
                src: SOURCE,
                status: resp.status,
            });
        }
        let json: Value = decode(&resp.body)?;
        let token = json
            .get("token")
            .and_then(Value::as_str)
            .ok_or_else(|| SubtitleError::Decode {
                src: SOURCE,
                detail: "login response missing token".to_string(),
            })?
            .to_string();
        *guard = Some(token.clone());
        Ok(token)
    }
}

#[async_trait]
impl<F: Fetcher> SubtitleProvider for OpenSubtitles<F> {
    fn name(&self) -> &'static str {
        SOURCE
    }

    async fn search(&self, query: &SubtitleQuery) -> Result<Vec<SubtitleMatch>, SubtitleError> {
        if self.config.api_key.is_empty() {
            return Err(SubtitleError::NoCredential { src: SOURCE });
        }
        let mut params: Vec<(String, String)> = Vec::new();
        if !query.languages.is_empty() {
            params.push(("languages".into(), query.languages.join(",")));
        }
        // TV searches key off the SERIES id + season/episode (the data model
        // carries series-level external ids); movies key off the film's own id.
        let is_tv = query.media_type == Some(MediaType::Tv);
        if let Some(imdb) = query.imdb_id.as_deref().map(strip_tt) {
            params.push((if is_tv { "parent_imdb_id" } else { "imdb_id" }.into(), imdb));
        } else if let Some(tmdb) = query.tmdb_id.as_deref() {
            params.push((
                if is_tv { "parent_tmdb_id" } else { "tmdb_id" }.into(),
                tmdb.to_string(),
            ));
        } else if let Some(q) = query.query.as_deref() {
            params.push(("query".into(), q.to_string()));
        }
        if is_tv {
            if let Some(s) = query.season {
                params.push(("season_number".into(), s.to_string()));
            }
            if let Some(e) = query.episode {
                params.push(("episode_number".into(), e.to_string()));
            }
        }
        let url = format!(
            "{}/api/v1/subtitles?{}",
            self.config.base_url,
            encode_query(&params)
        );
        let resp = self.fetcher.get(&url, &self.api_headers()).await?;
        if resp.status == 404 {
            return Ok(Vec::new());
        }
        if !resp.is_success() {
            return Err(SubtitleError::Http {
                src: SOURCE,
                status: resp.status,
            });
        }
        let json: Value = decode(&resp.body)?;
        let items = json.get("data").and_then(Value::as_array);
        let Some(items) = items else {
            return Ok(Vec::new());
        };
        Ok(items.iter().filter_map(normalize_match).collect())
    }

    async fn download(&self, m: &SubtitleMatch) -> Result<Vec<u8>, SubtitleError> {
        let token = self.bearer().await?;
        let auth = format!("Bearer {token}");
        let mut headers = self.api_headers();
        headers.push(("Authorization", auth.as_str()));

        let url = format!("{}/api/v1/download", self.config.base_url);
        let file_id: Value = m
            .id
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| Value::from(m.id.clone()));
        let body = serde_json::json!({ "file_id": file_id });
        let resp = self.fetcher.post_json(&url, &body, &headers).await?;
        if !resp.is_success() {
            return Err(SubtitleError::Http {
                src: SOURCE,
                status: resp.status,
            });
        }
        let json: Value = decode(&resp.body)?;
        let link = json
            .get("link")
            .and_then(Value::as_str)
            .ok_or_else(|| SubtitleError::Decode {
                src: SOURCE,
                detail: "download response missing link".to_string(),
            })?;
        // The link is a one-shot CDN URL; fetch the raw subtitle bytes.
        let file = self.fetcher.get(link, &[]).await?;
        if !file.is_success() {
            return Err(SubtitleError::Http {
                src: SOURCE,
                status: file.status,
            });
        }
        Ok(file.body)
    }
}

/// Turn one `data[]` entry into a [`SubtitleMatch`], or drop it when it carries no
/// downloadable file.
fn normalize_match(item: &Value) -> Option<SubtitleMatch> {
    let attrs = item.get("attributes")?;
    let language = attrs.get("language").and_then(Value::as_str)?.to_string();
    // The downloadable unit is the first file's numeric `file_id`.
    let file = attrs.get("files").and_then(Value::as_array)?.first()?;
    let file_id = file.get("file_id").and_then(Value::as_i64)?;
    let format = file
        .get("file_name")
        .and_then(Value::as_str)
        .and_then(|n| n.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()))
        .unwrap_or_else(|| "srt".to_string());
    Some(SubtitleMatch {
        provider: SOURCE,
        id: file_id.to_string(),
        language,
        release_name: attrs
            .get("release")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        forced: attrs
            .get("foreign_parts_only")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        hearing_impaired: attrs
            .get("hearing_impaired")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        score: rank(attrs),
        format,
    })
}

/// Fold OpenSubtitles' popularity signals into one comparable integer: rating
/// (0–10 → 0–100), download volume (capped), and a trust bonus.
fn rank(attrs: &Value) -> i32 {
    let ratings = attrs.get("ratings").and_then(Value::as_f64).unwrap_or(0.0);
    let downloads = attrs
        .get("download_count")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let trusted = attrs
        .get("from_trusted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let rating_pts = (ratings * 10.0).round() as i32;
    let download_pts = (downloads.clamp(0, 50_000) / 500) as i32;
    rating_pts + download_pts + i32::from(trusted) * 25
}

/// Strip a leading `tt` from an IMDb id — OpenSubtitles wants the numeric part.
fn strip_tt(id: &str) -> String {
    id.strip_prefix("tt").unwrap_or(id).to_string()
}

fn decode(body: &[u8]) -> Result<Value, SubtitleError> {
    serde_json::from_slice(body).map_err(|e| SubtitleError::Decode {
        src: SOURCE,
        detail: e.to_string(),
    })
}

/// Percent-encode a set of query params into `k=v&k=v`. Only the value needs
/// encoding (keys here are fixed ASCII); we encode everything outside the
/// unreserved set so a title `query` with spaces/punctuation is safe.
fn encode_query(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{k}={}", percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b',' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
