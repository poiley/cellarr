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

/// Serve an embedded asset by path.
///
/// Resolution order, matching a Next.js `output: 'export'` tree where each route
/// is emitted as `<route>/index.html`:
/// 1. the exact path (real assets: `_next/...`, `favicon.ico`, …);
/// 2. for an extensionless path, `<path>/index.html` (the per-route page, so
///    `/library` serves `library/index.html` — not the dashboard fallback);
/// 3. root `index.html` as the SPA fallback for anything else.
///
/// Returns 404 only if even `index.html` is absent (the committed placeholder
/// prevents that).
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    // 1. Exact asset hit.
    if let Some(content) = Assets::get(path) {
        return asset_response(path, content.data.into_owned());
    }

    // 2. Extensionless route → its exported `<route>/index.html`.
    if !last_segment_has_extension(path) {
        let route_index = format!("{}/index.html", path.trim_end_matches('/'));
        if let Some(content) = Assets::get(&route_index) {
            return asset_response(&route_index, content.data.into_owned());
        }
    }

    // 3. SPA fallback to the root document.
    match Assets::get("index.html") {
        Some(content) => asset_response("index.html", content.data.into_owned()),
        None => (StatusCode::NOT_FOUND, "asset not found").into_response(),
    }
}

/// Whether the final path segment looks like a file (has a `.ext`), so we don't
/// rewrite `_next/app.js` into `_next/app.js/index.html`.
fn last_segment_has_extension(path: &str) -> bool {
    path.rsplit('/').next().is_some_and(|seg| seg.contains('.'))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensionless_detection() {
        assert!(!last_segment_has_extension("library"));
        assert!(!last_segment_has_extension("settings/"));
        assert!(last_segment_has_extension("_next/static/app.js"));
        assert!(last_segment_has_extension("favicon.ico"));
    }

    // The built UI is embedded from web/dist; if the static export is present,
    // an extensionless route must resolve to its own `<route>/index.html`, not
    // the root document. This guards the per-route serving that a plain SPA
    // fallback would silently break (every screen showing the dashboard).
    #[tokio::test]
    async fn extensionless_route_serves_its_own_page_when_built() {
        // Only meaningful once the real UI is built into web/dist.
        let (Some(route), Some(root)) =
            (Assets::get("library/index.html"), Assets::get("index.html"))
        else {
            return;
        };
        if route.data == root.data {
            return; // placeholder build: nothing to distinguish
        }
        let resp = serve("/library".parse::<Uri>().unwrap()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            body.as_ref(),
            route.data.as_ref(),
            "/library must serve library/index.html, not the root SPA fallback"
        );
    }
}
