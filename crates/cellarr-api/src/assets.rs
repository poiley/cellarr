//! Embedded static frontend assets.
//!
//! The built SRCL UI (docs/10-ui.md) is embedded via `rust-embed` so the single
//! binary serves it too (docs/09-api.md). The asset directory is `web/dist`; a
//! committed placeholder `index.html` ensures the crate builds and serves a
//! "UI not built yet" page before the real frontend lands. Any path that is not
//! a known asset falls back to `index.html` so a client-side-routed SPA works.

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

/// The embedded `web/dist` tree. `folder` is relative to this crate's
/// `Cargo.toml`. It must tolerate an unbuilt UI: the committed placeholder is
/// the minimum, and the real assets land in a later phase.
#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct Assets;

/// Serve an embedded asset by path, falling back to `index.html` for unknown
/// paths (SPA routing). Returns 404 only if even `index.html` is absent, which
/// the committed placeholder prevents.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(content) => asset_response(path, content.data.into_owned()),
        None => match Assets::get("index.html") {
            Some(content) => asset_response("index.html", content.data.into_owned()),
            None => (StatusCode::NOT_FOUND, "asset not found").into_response(),
        },
    }
}

/// Build a response with a content type guessed from the file extension.
fn asset_response(path: &str, body: Vec<u8>) -> Response {
    let mime = mime_for(path);
    ([(header::CONTENT_TYPE, mime)], Body::from(body)).into_response()
}

/// A small extension → MIME map covering the asset kinds a Next.js export emits.
/// Deliberately tiny: no extra dependency for a handful of well-known types.
fn mime_for(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "map" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
