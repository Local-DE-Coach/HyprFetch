//! Browser-extension bridge (v0.6.1).
//!
//! The HyprFetch extension (Chrome/Chromium + Firefox, MV3) watches every
//! tab for media responses — the same trick IDM's browser integration uses:
//! inspect `Content-Type` / size from `webRequest`, count findings on the
//! toolbar badge, and hand picks to the local daemon over `127.0.0.1`.
//!
//! This module is the daemon side of that handshake:
//!
//! | route                        | purpose                                     |
//! |------------------------------|---------------------------------------------|
//! | `POST /api/extension/heartbeat` | extension liveness + version             |
//! | `POST /api/extension/media`  | extension reports media found in a tab      |
//! | `GET  /api/extension/media`  | WebUI lists captured media                  |
//! | `DELETE /api/extension/media`| WebUI clears the captured list              |
//! | `POST /api/extension/download` | extension → start a download (source=extension) |
//! | `GET  /api/extension/status` | WebUI: is the extension connected?          |
//!
//! Everything lives on loopback; the CORS shim exists so browser-extension
//! origins (`chrome-extension://…`, `moz-extension://…`) can call these
//! routes even from a page context.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::{error::ApiError, routes, AppState};

/// How long after the last heartbeat we still call the extension "connected".
const HEARTBEAT_TTL: Duration = Duration::from_secs(90);

/// Ring-buffer cap for captured media (memory safety on tab-happy days).
const MEDIA_CAP: usize = 300;

/// One media item the extension saw in a tab.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionMedia {
    pub url: String,
    /// `video/mp4`, `audio/webm`, `image/jpeg`, … (as reported).
    #[serde(default)]
    pub media_type: Option<String>,
    /// Content-Length when the response carried one.
    #[serde(default)]
    pub size: Option<u64>,
    /// Best-effort filename (Content-Disposition → URL tail).
    #[serde(default)]
    pub filename: Option<String>,
    /// Page that produced the media request.
    #[serde(default)]
    pub page_url: Option<String>,
    /// Page title when the extension could grab it.
    #[serde(default)]
    pub page_title: Option<String>,
    /// Millis timestamp of the sighting.
    #[serde(default)]
    pub ts: i64,
}

/// Shared daemon-side state for the extension bridge.
#[derive(Default)]
pub struct ExtensionState {
    last_seen: Mutex<Option<i64>>,
    version: Mutex<Option<String>>,
    media: Mutex<Vec<ExtensionMedia>>,
}

impl ExtensionState {
    pub fn new() -> Self {
        Self::default()
    }

    fn heartbeat(&self, version: Option<String>) {
        *self.last_seen.lock().unwrap() = Some(now_ms());
        if let Some(v) = version.filter(|s| !s.trim().is_empty()) {
            *self.version.lock().unwrap() = Some(v);
        }
    }

    fn merge_media(&self, items: Vec<ExtensionMedia>) -> usize {
        let mut media = self.media.lock().unwrap();
        let mut added = 0usize;
        for item in items {
            if item.url.is_empty() {
                continue;
            }
            // Dedup by URL — refresh the entry (new ts, new size) instead of
            // piling duplicates when the page re-requests the same asset.
            if let Some(existing) = media.iter_mut().find(|m| m.url == item.url) {
                existing.ts = item.ts;
                if item.size.is_some() {
                    existing.size = item.size;
                }
                if item.media_type.is_some() {
                    existing.media_type = item.media_type;
                }
                continue;
            }
            media.insert(0, item); // newest first
            added += 1;
        }
        if media.len() > MEDIA_CAP {
            media.truncate(MEDIA_CAP);
        }
        added
    }

    fn connected(&self) -> bool {
        match *self.last_seen.lock().unwrap() {
            Some(ts) => now_ms() - ts <= HEARTBEAT_TTL.as_millis() as i64,
            None => false,
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /api/extension/heartbeat` — extension calls this every ~30s and on
/// startup; the WebUI "Extension" page reads the freshness via `/status`.
pub async fn heartbeat(
    State(state): State<AppState>,
    Json(req): Json<HeartbeatRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.extension.heartbeat(req.version);
    Ok(Json(serde_json::json!({
        "ok": true,
        "server_time": now_ms(),
    })))
}

#[derive(Debug, Deserialize, Default)]
pub struct HeartbeatRequest {
    pub version: Option<String>,
}

/// `POST /api/extension/media` — the extension ships a batch of sightings
/// (one call per tab update; batches keep the protocol chatty-cheap).
pub async fn report_media(
    State(state): State<AppState>,
    Json(req): Json<ReportMediaRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.extension.heartbeat(None);
    let ts = now_ms();
    let items: Vec<ExtensionMedia> = req
        .media
        .into_iter()
        .map(|mut m| {
            if m.ts == 0 {
                m.ts = ts;
            }
            m
        })
        .collect();
    let added = state.extension.merge_media(items);
    Ok(Json(serde_json::json!({ "ok": true, "added": added })))
}

#[derive(Debug, Deserialize, Default)]
pub struct ReportMediaRequest {
    #[serde(default)]
    pub media: Vec<ExtensionMedia>,
}

/// `GET /api/extension/media` — the WebUI Extension page's media list.
pub async fn list_media(State(state): State<AppState>) -> Json<serde_json::Value> {
    let media = state.extension.media.lock().unwrap().clone();
    Json(serde_json::json!({
        "items": media,
        "count": media.len(),
    }))
}

/// `DELETE /api/extension/media` — clear the captured list (trash button).
pub async fn clear_media(State(state): State<AppState>) -> Json<serde_json::Value> {
    state.extension.media.lock().unwrap().clear();
    Json(serde_json::json!({ "ok": true }))
}

/// `POST /api/extension/download` — the extension (or the WebUI Extension
/// page, same route) hands a URL to the daemon. Creates a task tagged
/// `source = "extension"` so the Tasks page can filter on it.
pub async fn download(
    State(state): State<AppState>,
    Json(req): Json<ExtensionDownloadRequest>,
) -> Result<(StatusCode, Json<routes::TaskDto>), ApiError> {
    state.extension.heartbeat(None);
    let url = req.url.trim().to_string();
    if url.is_empty() {
        return Err(ApiError::InvalidRequest("url must not be empty".into()));
    }
    routes::create_extension_task(
        &state,
        &url,
        req.filename.as_deref(),
        req.page_url.as_deref(),
    )
    .await
    .map(|dto| (StatusCode::CREATED, Json(dto)))
}

#[derive(Debug, Deserialize)]
pub struct ExtensionDownloadRequest {
    pub url: String,
    pub filename: Option<String>,
    pub page_url: Option<String>,
}

/// `GET /api/extension/status` — connected + last heartbeat + count.
pub async fn status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let connected = state.extension.connected();
    let last_seen = *state.extension.last_seen.lock().unwrap();
    let version = state.extension.version.lock().unwrap().clone();
    let count = state.extension.media.lock().unwrap().len();
    Json(serde_json::json!({
        "connected": connected,
        "last_seen": last_seen,
        "version": version,
        "media_count": count,
    }))
}

// ---------------------------------------------------------------------------
// CORS shim for extension origins (scoped to the /api/extension router)
// ---------------------------------------------------------------------------

/// Permissive CORS for the extension bridge only. The extension's host
/// permissions already exempt it from page CORS, but Firefox/Chrome page
/// contexts and curl-style clients benefit from explicit headers, and the
/// preflight answer keeps `POST application/json` working everywhere.
pub async fn cors(req: Request<axum::body::Body>, next: axum::middleware::Next) -> Response {
    if req.method() == axum::http::Method::OPTIONS {
        let mut res = StatusCode::OK.into_response();
        apply_cors(&mut res);
        return res;
    }
    let mut res = next.run(req).await;
    apply_cors(&mut res);
    res
}

fn apply_cors(res: &mut Response) {
    let headers = res.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, DELETE, OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Content-Type"),
    );
    headers.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("86400"),
    );
}
