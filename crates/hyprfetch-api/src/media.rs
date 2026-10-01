//! Media-engine API surface (v0.6.1) — universal "download any media"
//! endpoints driving the yt-dlp integration in `hyprfetch-core::media`.
//!
//! | route                        | purpose                                        |
//! |------------------------------|------------------------------------------------|
//! | `POST /api/media/probe`      | URL → kind (file/media) + quality ladder       |
//! | `POST /api/media/download`   | start a quality-picked media task              |
//! | `GET  /api/media/ytdlp`      | yt-dlp install status (path + version)         |
//! | `POST /api/media/ytdlp/install` | install / re-install / update the binary    |
//!
//! The probe answers BOTH worlds: direct files (the LinkedIn-logo class —
//! extension-less URLs that are actually `image/jpeg` blobs) stay on the
//! native HTTP engine, while stream pages (YouTube, Vimeo, …) come back
//! with a de-duplicated quality ladder.

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use hyprfetch_core::media::{
    distill, ensure_ytdlp, find_ytdlp, has_ffmpeg, probe_url, quality_ladder_with,
    title_to_filename, version_of, MediaError, MediaProbe, QualityOption,
};

use crate::error::ApiError;
use crate::routes::{self, resolve_save_dir};
use crate::AppState;

// ---------------------------------------------------------------------------
// POST /api/media/probe
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct MediaProbeRequest {
    pub url: String,
}

/// Unified probe response: `kind = "file"` mirrors `POST /api/inspect`
/// (native engine), `kind = "media"` carries the quality ladder.
#[derive(Serialize)]
pub struct UnifiedProbeResponse {
    pub kind: &'static str, // "file" | "media"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaProbe>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<routes::InspectResponse>,
    /// yt-dlp version actually used (media probes only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ytdlp_version: Option<String>,
}

pub async fn media_probe(
    State(state): State<AppState>,
    Json(req): Json<MediaProbeRequest>,
) -> Result<Json<UnifiedProbeResponse>, ApiError> {
    let url = req.url.trim().to_string();
    routes::validate_public_url(&url)?;

    // 1. Native HTTP probe first — cheap and authoritative for direct files.
    let native = state
        .engine
        .inspect_url(&url::Url::parse(&url).map_err(|e| ApiError::InvalidUrl(e.to_string()))?)
        .await;

    let direct_file = match native {
        Ok(probe) => {
            let ct = probe.content_type.clone().unwrap_or_default();
            let is_html = ct.starts_with("text/html") || ct.starts_with("application/xhtml");
            // No Content-Type at all + no length → almost certainly a page
            // or a stream we don't understand; let yt-dlp try.
            let looks_like_file =
                !is_html && (probe.content_type.is_some() || probe.content_length.is_some());
            if looks_like_file {
                Some(probe)
            } else {
                None
            }
        }
        Err(_) => None, // native probe failed → media engine gets a chance
    };

    if let Some(probe) = direct_file {
        let file = routes::inspect_response_from_probe(&state, &url, probe)?;
        return Ok(Json(UnifiedProbeResponse {
            kind: "file",
            media: None,
            file: Some(file),
            ytdlp_version: None,
        }));
    }

    // 2. Media engine.
    let binary = ensure_ytdlp()
        .await
        .map_err(media_api_err("yt-dlp is not available"))?;
    let cookies = hyprfetch_core::media::cookies_args_from_settings(&state.db);
    let info = probe_url(&binary, &url, &cookies)
        .await
        .map_err(media_api_err("media probe failed"))?;
    let version = version_of(&binary).await.ok();

    Ok(Json(UnifiedProbeResponse {
        kind: "media",
        media: Some(distill(&info, has_ffmpeg())),
        file: None,
        ytdlp_version: version,
    }))
}

// ---------------------------------------------------------------------------
// POST /api/media/download
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct MediaDownloadRequest {
    pub url: String,
    /// Quality id from the probe (`137+140`, `22`, `bestaudio`, …).
    /// Omitted / `"best"` → the top of the ladder.
    pub quality: Option<String>,
    /// Force the audio-only option (MP3).
    pub audio_only: Option<bool>,
    /// Optional filename override (stem; container extension appended).
    pub filename: Option<String>,
    pub save_dir: Option<String>,
    pub category: Option<String>,
}

pub async fn media_download(
    State(state): State<AppState>,
    Json(req): Json<MediaDownloadRequest>,
) -> Result<(axum::http::StatusCode, Json<routes::TaskDto>), ApiError> {
    let url = req.url.trim().to_string();
    routes::validate_public_url(&url)?;

    // Probe (re-probe server-side; clients cannot be trusted for formats).
    let binary = ensure_ytdlp()
        .await
        .map_err(media_api_err("yt-dlp is not available"))?;
    let cookies = hyprfetch_core::media::cookies_args_from_settings(&state.db);
    let info = probe_url(&binary, &url, &cookies)
        .await
        .map_err(media_api_err("media probe failed"))?;
    let ladder: Vec<QualityOption> = quality_ladder_with(&info, has_ffmpeg());
    if ladder.is_empty() {
        return Err(ApiError::InvalidRequest(
            "no downloadable formats found for this URL".into(),
        ));
    }

    let want_audio = req.audio_only.unwrap_or(false);
    let chosen = if want_audio {
        ladder
            .iter()
            .find(|q| q.audio_only)
            .ok_or_else(|| ApiError::InvalidRequest("no audio stream available".into()))?
    } else {
        let want = req.quality.as_deref().unwrap_or("best");
        ladder
            .iter()
            .filter(|q| !q.audio_only)
            .find(|q| q.id == want)
            .or_else(|| ladder.first())
            .expect("ladder non-empty")
    };

    // Title → filename.
    let title = info
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("media")
        .to_string();
    let stem = req
        .filename
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(title_to_filename)
        .unwrap_or_else(|| title_to_filename(&title));
    let filename = format!("{stem}.{}", chosen.container);

    // Destination (same rules as HTTP tasks: direct dir > category > auto).
    let settings_map: std::collections::BTreeMap<String, String> =
        hyprfetch_db::SettingsRepo::new(&state.db)
            .all()?
            .into_iter()
            .collect();
    let save_dir = resolve_save_dir(
        req.save_dir.as_deref(),
        req.category.as_deref(),
        &filename,
        &settings_map,
    )?;
    if let Err(e) = std::fs::create_dir_all(&save_dir) {
        tracing::warn!(dir = %save_dir, error = %e, "could not pre-create save dir");
    }
    let save_path = format!("{save_dir}/{filename}");

    let media_meta = serde_json::json!({
        "selector": chosen.id,
        "audio_only": chosen.audio_only,
        "extract_mp3": chosen.audio_only && chosen.container == "mp3",
        "quality": chosen.label,
        "container": chosen.container,
        "extractor": info.get("extractor_key").and_then(|v| v.as_str()).unwrap_or(""),
        "title": title,
        "requested_url": url,
    })
    .to_string();

    let dto = routes::create_media_task(&state, &url, &filename, &save_path, media_meta).await?;
    Ok((axum::http::StatusCode::CREATED, Json(dto)))
}

// ---------------------------------------------------------------------------
// GET /api/media/ytdlp + POST /api/media/ytdlp/install
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct YtdlpStatus {
    pub installed: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    /// ffmpeg presence — merges + MP3 extraction depend on it.
    pub ffmpeg: bool,
}

pub async fn ytdlp_status() -> Json<YtdlpStatus> {
    let ffmpeg = has_ffmpeg();
    match find_ytdlp() {
        Some(path) => {
            let version = version_of(&path).await.ok();
            Json(YtdlpStatus {
                installed: version.is_some(),
                path: Some(path.to_string_lossy().into_owned()),
                version,
                ffmpeg,
            })
        }
        None => Json(YtdlpStatus {
            installed: false,
            path: None,
            version: None,
            ffmpeg,
        }),
    }
}

pub async fn ytdlp_install() -> Result<Json<YtdlpStatus>, ApiError> {
    let path = hyprfetch_core::media::install_ytdlp()
        .await
        .map_err(media_api_err("yt-dlp install failed"))?;
    let version = version_of(&path).await.ok();
    Ok(Json(YtdlpStatus {
        installed: true,
        path: Some(path.to_string_lossy().into_owned()),
        version,
        ffmpeg: has_ffmpeg(),
    }))
}

/// Map a media-engine error to a user-visible API error.
fn media_api_err(prefix: &'static str) -> impl Fn(MediaError) -> ApiError {
    move |e: MediaError| match e {
        MediaError::InstallFailed(_) => ApiError::ServiceUnavailable(format!("{prefix}: {e}")),
        other => ApiError::InvalidRequest(format!("{prefix}: {other}")),
    }
}
