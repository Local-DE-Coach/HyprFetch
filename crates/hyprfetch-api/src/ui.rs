//! Embedded web UI (Svelte SPA built into `ui/dist`, embedded at compile
//! time via `rust-embed`).
//!
//! Routing rules:
//! - `/` → `index.html`
//! - `/assets/*` (hashed files) → served with long-lived caching
//! - any other extension-less path → `index.html` (SPA deep-link fallback)
//! - unknown `/api/*` or `/ws` paths → JSON/plain 404, NOT the SPA

use axum::http::uri::Uri;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

/// Compiled SPA assets. The folder is resolved relative to this crate's
/// manifest at compile time; `ui/dist` is committed so `cargo build` works
/// without a Node toolchain (rebuild with `npm run build` after UI changes).
#[derive(RustEmbed)]
#[folder = "ui/dist"]
struct UiAssets;

fn serve(path: &str) -> Response {
    match UiAssets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let cache = if path.starts_with("assets/") {
                // Vite emits content-hashed filenames — cache forever.
                "public, max-age=31536000, immutable"
            } else {
                "no-cache" // index.html: always revalidate for new deploys
            };
            (
                [
                    (header::CONTENT_TYPE, mime.as_ref().to_string()),
                    (header::CACHE_CONTROL, cache.to_string()),
                ],
                file.data,
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// `GET /` — the SPA entry point.
pub async fn index() -> Response {
    serve("index.html")
}

/// Fallback for every other unmatched path: real assets are served, deep
/// links fall back to the SPA, API namespaces get a proper 404.
pub async fn static_path(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    if path.is_empty() {
        return serve("index.html");
    }
    // Don't shadow API error semantics with the SPA fallback.
    if path == "ws" || path.starts_with("api/") || path.starts_with("healthz") {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    // Extension-less path → client-side route → SPA.
    if !path.contains('.') {
        return serve("index.html");
    }
    serve(path)
}

#[cfg(test)]
pub(crate) fn asset_names() -> Vec<String> {
    UiAssets::iter().map(|p| p.to_string()).collect()
}
