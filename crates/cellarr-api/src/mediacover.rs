//! The `MediaCover` artwork route (`GET /api/v3/mediacover/{contentId}/{kind}`).
//!
//! Sonarr/Radarr serve cached poster/fanart through a `MediaCover/{id}/{kind}`
//! path; the ecosystem (Overseerr, dashboards, the cellarr UI) loads artwork from
//! there. cellarr mirrors it: the identify/refresh path caches an item's poster
//! and fanart bytes under `<data_dir>/MediaCover/<contentId>/<kind>.<ext>`, and
//! this route streams those bytes back with the right content type, or a 404 when
//! the item has no cached artwork of that kind.
//!
//! The route is read-only and open (artwork is not sensitive), matching the
//! originals. Path traversal is impossible: the id must parse as a UUID and the
//! kind must be one of the two known slugs, so neither path segment can contain a
//! separator or `..`.

use std::path::{Path, PathBuf};

use axum::extract::{Path as AxumPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::state::AppState;

/// The artwork kinds the route serves (the normalized schema's two image kinds).
const KINDS: [&str; 2] = ["poster", "fanart"];

/// The on-disk path an item's artwork of `kind` is cached at, under `dir`, if any
/// file is present. Tries the known image extensions in order and returns the
/// first that exists (the cache writer picks the extension from the source URL,
/// so the reader must not assume one). Returns `None` when no file is present.
///
/// `content_id` is the already-validated UUID string and `kind` one of [`KINDS`];
/// both are free of path separators, so the join cannot escape `dir`.
#[must_use]
pub fn cached_artwork_path(dir: &Path, content_id: &str, kind: &str) -> Option<PathBuf> {
    let base = dir.join(content_id);
    for ext in ["jpg", "jpeg", "png", "webp"] {
        let candidate = base.join(format!("{kind}.{ext}"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Guess the `Content-Type` for a cached artwork file from its extension. Defaults
/// to `image/jpeg` (the dominant artwork format) for an unknown extension.
fn content_type_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        // jpg/jpeg and anything else.
        _ => "image/jpeg",
    }
}

/// `GET /api/v3/mediacover/{contentId}/{kind}` — serve a cached artwork image.
///
/// Returns the image bytes with the matching content type on a cache hit; a 404
/// when the kind is unknown, no artwork dir is configured, the id is malformed, or
/// no file is cached for that item/kind.
pub async fn media_cover(
    State(state): State<AppState>,
    AxumPath((content_id, kind)): AxumPath<(String, String)>,
) -> Response {
    // An unknown kind is a 404, not a 400: the route only knows the two slugs.
    if !KINDS.contains(&kind.as_str()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    // A malformed id is also a 404 (there is simply no such artwork), and the
    // parse guarantees the id segment carries no path separator.
    if content_id.parse::<uuid::Uuid>().is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(dir) = state.artwork_dir.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(path) = cached_artwork_path(dir, &content_id, &kind) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let content_type = content_type_for(&path);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, content_type)],
            bytes,
        )
            .into_response(),
        // A file that existed at the stat but cannot be read is treated as absent.
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_first_existing_extension() {
        let tmp = std::env::temp_dir().join(format!("cellarr-mc-{}", uuid::Uuid::new_v4()));
        let id = uuid::Uuid::new_v4().to_string();
        let item = tmp.join(&id);
        std::fs::create_dir_all(&item).unwrap();
        std::fs::write(item.join("poster.png"), b"x").unwrap();
        let found = cached_artwork_path(&tmp, &id, "poster").unwrap();
        assert_eq!(found, item.join("poster.png"));
        assert!(cached_artwork_path(&tmp, &id, "fanart").is_none());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn content_type_matches_extension() {
        assert_eq!(content_type_for(Path::new("a/poster.png")), "image/png");
        assert_eq!(content_type_for(Path::new("a/poster.jpg")), "image/jpeg");
        assert_eq!(content_type_for(Path::new("a/poster.webp")), "image/webp");
        assert_eq!(content_type_for(Path::new("a/poster")), "image/jpeg");
    }
}
