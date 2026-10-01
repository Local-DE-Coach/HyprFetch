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
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use hyprfetch_core::media::{
    distill, ensure_ytdlp, find_ytdlp, has_ffmpeg, probe_url, quality_ladder_with,
    title_to_filename, version_of, MediaError, MediaProbe, QualityOption,
};

use crate::error::ApiError;
use crate::routes::{self, resolve_save_dir};
use crate::AppState;

// ---------------------------------------------------------------------------
// Probe cache — the "extension is slow to list qualities" fix
// ---------------------------------------------------------------------------
// Every popup open / quality panel used to spawn a fresh `yt-dlp -J` (2–6 s
// on YouTube). v0.6.4 caches the distilled probe per URL (15 min TTL, LRU-
// capped) and deduplicates concurrent requests for the same URL, so the
// extension's background prefetch + popup + the download endpoint all share
// ONE extraction per video.

const PROBE_TTL: Duration = Duration::from_secs(15 * 60);
const PROBE_CACHE_CAP: usize = 64;

struct CachedProbe {
    info: Arc<serde_json::Value>,
    /// Pre-distilled ladder — lets the probe endpoint answer a media-page
    /// hit WITHOUT re-running the cheap-but-not-free native HEAD probe.
    media: Option<MediaProbe>,
    version: Option<String>,
}

type ProbeCache = HashMap<String, (Instant, Arc<CachedProbe>)>;

fn probe_cache() -> &'static Mutex<ProbeCache> {
    static CACHE: OnceLock<Mutex<ProbeCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn inflight() -> &'static Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>> {
    static INFLIGHT: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
        OnceLock::new();
    INFLIGHT.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_get(url: &str) -> Option<Arc<CachedProbe>> {
    let cache = probe_cache().lock().expect("probe cache");
    let hit = cache.get(url).filter(|(at, _)| at.elapsed() < PROBE_TTL)?;
    Some(hit.1.clone())
}

fn cache_put(url: &str, entry: CachedProbe) {
    let mut cache = probe_cache().lock().expect("probe cache");
    if cache.len() >= PROBE_CACHE_CAP {
        // Drop the oldest entry — good-enough LRU for a desktop daemon.
        if let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, (at, _))| *at)
            .map(|(k, _)| k.clone())
        {
            cache.remove(&oldest);
        }
    }
    cache.insert(url.to_string(), (Instant::now(), Arc::new(entry)));
}

/// Cached + single-flighted `yt-dlp -J`: concurrent callers for the same URL
/// wait for the first one instead of stacking extraction processes.
async fn get_or_probe(
    binary: &std::path::Path,
    url: &str,
    cookies: &[String],
    ffmpeg: bool,
) -> Result<Arc<CachedProbe>, MediaError> {
    if let Some(hit) = cache_get(url) {
        return Ok(hit);
    }
    let gate = inflight()
        .lock()
        .expect("inflight")
        .entry(url.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = gate.lock().await;
    // Another request may have filled the cache while we waited.
    if let Some(hit) = cache_get(url) {
        return Ok(hit);
    }
    let info = probe_url(binary, url, cookies).await?;
    let version = version_of(binary).await.ok();
    let media = distill(&info, ffmpeg);
    cache_put(
        url,
        CachedProbe {
            info: Arc::new(info),
            media: Some(media),
            version: version.clone(),
        },
    );
    inflight().lock().expect("inflight").remove(url);
    Ok(cache_get(url).expect("just inserted"))
}

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

    // 0. Media-page cache hit → answer instantly (the extension prefetch +
    // repeat popup/panel opens land here; nothing runs at all).
    if let Some(hit) = cache_get(&url) {
        if let Some(media) = hit.media.clone() {
            return Ok(Json(UnifiedProbeResponse {
                kind: "media",
                media: Some(media),
                file: None,
                ytdlp_version: hit.version.clone(),
            }));
        }
    }

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
    let cached = get_or_probe(&binary, &url, &cookies, has_ffmpeg())
        .await
        .map_err(media_api_err("media probe failed"))?;

    Ok(Json(UnifiedProbeResponse {
        kind: "media",
        media: cached.media.clone(),
        file: None,
        ytdlp_version: cached.version.clone(),
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

    // Cached probe (same cache as /api/media/probe): a download click after
    // browsing the panel no longer re-extracts the whole format list.
    let binary = ensure_ytdlp()
        .await
        .map_err(media_api_err("yt-dlp is not available"))?;
    let cookies = hyprfetch_core::media::cookies_args_from_settings(&state.db);
    let cached = get_or_probe(&binary, &url, &cookies, has_ffmpeg())
        .await
        .map_err(media_api_err("media probe failed"))?;
    let info = &cached.info;
    let ladder: Vec<QualityOption> = quality_ladder_with(info, has_ffmpeg());
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

    // Refuse a second task for the same destination while one is queued or
    // downloading — two yt-dlp processes on one output file race on their
    // fragment files and die with a cryptic rename error (v0.6.4 fix).
    {
        let repo = hyprfetch_db::TasksRepo::new(&state.db);
        let busy = [
            hyprfetch_db::schema::TaskState::Queued,
            hyprfetch_db::schema::TaskState::Downloading,
        ]
        .iter()
        .any(|s| {
            repo.list_by_state(Some(*s))
                .map(|rows| rows.iter().any(|t| t.save_path == save_path))
                .unwrap_or(false)
        });
        if busy {
            return Err(ApiError::InvalidStateTransition(format!(
                "a download to \"{filename}\" is already queued or running — pick it up in the task list"
            )));
        }
    }

    let media_meta = serde_json::json!({
        "selector": chosen.id,
        "audio_only": chosen.audio_only,
        "extract_mp3": chosen.audio_only && chosen.container == "mp3",
        "quality": chosen.label,
        "container": chosen.container,
        "size": chosen.size_bytes,
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
    /// JS runtime presence — modern yt-dlp needs one (deno) for YouTube's
    /// full format list and unthrottled transfer speeds.
    pub deno: bool,
}

pub async fn ytdlp_status() -> Json<YtdlpStatus> {
    let ffmpeg = has_ffmpeg();
    let deno = hyprfetch_core::media::find_deno().is_some();
    match find_ytdlp() {
        Some(path) => {
            let version = version_of(&path).await.ok();
            Json(YtdlpStatus {
                installed: version.is_some(),
                path: Some(path.to_string_lossy().into_owned()),
                version,
                ffmpeg,
                deno,
            })
        }
        None => Json(YtdlpStatus {
            installed: false,
            path: None,
            version: None,
            ffmpeg,
            deno,
        }),
    }
}

pub async fn ytdlp_install() -> Result<Json<YtdlpStatus>, ApiError> {
    let path = hyprfetch_core::media::install_ytdlp()
        .await
        .map_err(media_api_err("yt-dlp install failed"))?;
    // Same policy as auto-install: a JS runtime unlocks YouTube's full
    // ladder; a failed deno install must not fail the yt-dlp install.
    let _ = hyprfetch_core::media::ensure_deno().await;
    let version = version_of(&path).await.ok();
    Ok(Json(YtdlpStatus {
        installed: true,
        path: Some(path.to_string_lossy().into_owned()),
        version,
        ffmpeg: has_ffmpeg(),
        deno: hyprfetch_core::media::find_deno().is_some(),
    }))
}

/// Map a media-engine error to a user-visible API error.
fn media_api_err(prefix: &'static str) -> impl Fn(MediaError) -> ApiError {
    move |e: MediaError| match e {
        MediaError::InstallFailed(_) => ApiError::ServiceUnavailable(format!("{prefix}: {e}")),
        other => ApiError::InvalidRequest(format!("{prefix}: {other}")),
    }
}
