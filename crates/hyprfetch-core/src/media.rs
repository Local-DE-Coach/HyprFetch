//! Media engine — universal "download any media from any URL" (v0.6.1).
//!
//! HyprFetch's own HTTP engine is a segmented downloader: it is perfect for
//! direct file URLs but cannot extract streams from video pages (YouTube,
//! Vimeo, Twitter/X, TikTok, …) where the real media lives behind DASH/HLS
//! manifests and rotating signed URLs. Every serious download manager
//! (IDM, FDM) ships a separate extraction layer for exactly this reason —
//! so does HyprFetch: this module drives `yt-dlp`, the industry-standard
//! extraction engine, as a managed subprocess.
//!
//! Responsibilities:
//! 1. **Provisioning** — locate `yt-dlp` on `$PATH`, or auto-install the
//!    official static Linux build into `~/.local/share/hyprfetch/bin/`.
//!    The binary is mirrored on the self-hosted channel (istias.tech) so
//!    end users never touch GitHub (same policy as app updates).
//! 2. **Probing** — run `yt-dlp -J` against a URL and distill the raw
//!    format soup into a clean, de-duplicated quality ladder (one entry
//!    per resolution, MP4 preferred — the IDM/FDM convention the UI
//!    promises). Pure logic lives in [`quality_ladder`] and is unit-tested.
//! 3. **Downloading** — [`run_ytdlp_coordinator`] is spawned by the engine
//!    for tasks with `source = "media"`. It translates yt-dlp's progress
//!    stream into the same DB rows + events the segmented downloader uses,
//!    so the WebUI, widget and CLI all just work.
//!
//! Direct files (the LinkedIn logo URL case: no extension, query-string
//! name, `image/jpeg` behind a redirect) never reach yt-dlp — the API
//! layer first probes with the native HTTP engine and only falls back to
//! the media engine when the URL is not a plain file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use hyprfetch_db::schema::TaskState;
use hyprfetch_db::{SettingsRepo, TasksRepo};

use crate::categories::sanitize_filename;
use crate::events::{EngineEvent, EventBus};

/// Marker prefix for yt-dlp progress lines. yt-dlp prints one line per
/// progress update when `--newline` is set; everything after this prefix
/// is `downloaded|total|speed|eta` (values are `NA` when unknown).
pub const PROGRESS_PREFIX: &str = "@@HFPROGRESS@@";

/// Primary mirror of the official `yt-dlp-linux` build — the self-hosted
/// update channel. End users must never depend on GitHub reachability.
pub const YTDLP_MIRROR: &str = "https://istias.tech/hyprfetch/updates/bin/yt-dlp/yt-dlp-linux";

/// Fallback mirror (only tried when the primary mirror fails — dev boxes,
/// broken DNS, channel migration).
pub const YTDLP_FALLBACK: &str =
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux";

/// `yt-dlp -J` hard timeout. Heavy pages (YouTube with many formats) can
/// take a few seconds; 45s covers slow machines without hanging the API.
const PROBE_TIMEOUT: Duration = Duration::from_secs(45);

/// Where the managed binary lives.
pub fn managed_bin_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    Path::new(&home).join(".local/share/hyprfetch/bin")
}

/// Full path of the managed binary (`~/.local/share/hyprfetch/bin/yt-dlp`).
pub fn managed_bin_path() -> PathBuf {
    managed_bin_dir().join("yt-dlp")
}

/// Locate a usable `yt-dlp` binary: `$PATH` first (respects distro/pip
/// installs), then the managed copy.
pub fn find_ytdlp() -> Option<PathBuf> {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("yt-dlp");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let managed = managed_bin_path();
    if managed.is_file() {
        return Some(managed);
    }
    None
}

/// Errors surfaced by the media engine.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("yt-dlp is not installed and auto-install failed: {0}")]
    InstallFailed(String),
    #[error("probe failed: {0}")]
    Probe(String),
    #[error("download failed: {0}")]
    Download(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("db: {0}")]
    Db(#[from] rusqlite::Error),
}

/// Install the official static Linux build into the managed bin dir.
/// Tries the self-hosted mirror first, GitHub second. Returns the path.
pub async fn install_ytdlp() -> Result<PathBuf, MediaError> {
    let dest = managed_bin_path();
    std::fs::create_dir_all(dest.parent().unwrap())?;

    let mut last_err = String::from("no source attempted");
    for url in [YTDLP_MIRROR, YTDLP_FALLBACK] {
        info!(url, "downloading yt-dlp");
        match download_binary(url, &dest).await {
            Ok(()) => {
                // Smoke-test the binary before trusting it.
                match version_of(&dest).await {
                    Ok(_) => {
                        info!(path = %dest.display(), "yt-dlp installed");
                        return Ok(dest);
                    }
                    Err(e) => {
                        last_err = format!("downloaded binary failed to run: {e}");
                        warn!("{last_err}");
                        let _ = std::fs::remove_file(&dest);
                    }
                }
            }
            Err(e) => {
                last_err = format!("{url}: {e}");
                warn!("yt-dlp download failed: {last_err}");
            }
        }
    }
    Err(MediaError::InstallFailed(last_err))
}

async fn download_binary(url: &str, dest: &Path) -> Result<(), String> {
    let resp = reqwest::get(url)
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?;
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() < 1_000_000 {
        return Err(format!("suspiciously small binary ({} bytes)", bytes.len()));
    }
    std::fs::write(dest, &bytes).map_err(|e| e.to_string())?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Ensure a working yt-dlp exists: return the located path, or install.
pub async fn ensure_ytdlp() -> Result<PathBuf, MediaError> {
    if let Some(p) = find_ytdlp() {
        return Ok(p);
    }
    install_ytdlp().await
}

/// Locate `ffmpeg` on `$PATH` (yt-dlp shells out to it for merges and
/// audio extraction — its presence decides what the ladder can offer).
pub fn find_ffmpeg() -> Option<PathBuf> {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("ffmpeg");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Process-lifetime cached ffmpeg presence check.
pub fn has_ffmpeg() -> bool {
    static FFMPEG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FFMPEG.get_or_init(|| find_ffmpeg().is_some())
}

/// Run `yt-dlp --version` and return the reported version string.
pub async fn version_of(binary: &Path) -> Result<String, MediaError> {
    let out = tokio::process::Command::new(binary)
        .arg("--version")
        .output()
        .await
        .map_err(MediaError::Io)?;
    let text = String::from_utf8_lossy(&out.stdout);
    let v = text.lines().next().unwrap_or("").trim().to_string();
    if out.status.success() && !v.is_empty() {
        Ok(v)
    } else {
        Err(MediaError::Probe(format!("exit {:?}", out.status.code())))
    }
}

// ---------------------------------------------------------------------------
// Probe + quality ladder
// ---------------------------------------------------------------------------

/// One cleaned-up quality option (what the UI shows in the picker).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QualityOption {
    /// Opaque id the client sends back to start this quality
    /// (the yt-dlp format selector, e.g. `137+140`).
    pub id: String,
    /// Human label, e.g. `1080p` / `720p60` / `Audio only (MP3)`.
    pub label: String,
    /// Video height in px (None for audio-only).
    pub height: Option<u32>,
    /// Container we promise the file to land in (`mp4` / `mp3` / `webm`…).
    pub container: String,
    /// Best-effort total size (video + audio), when either reports one.
    pub size_bytes: Option<u64>,
    /// Friendly note (`4K`, `Full HD`, `HD`, `60fps`, `HV30` …).
    pub note: Option<String>,
    /// True for the audio-only option.
    pub audio_only: bool,
}

/// A distilled probe result (API response shape for `POST /api/media/probe`).
#[derive(Debug, Clone, Serialize)]
pub struct MediaProbe {
    pub kind: &'static str, // always "media" here
    pub extractor: Option<String>,
    pub title: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
    pub webpage_url: Option<String>,
    pub is_live: bool,
    pub qualities: Vec<QualityOption>,
    /// Suggested filename for the "best" option (no directory).
    pub suggested_filename: String,
    /// Whether ffmpeg is available for merging DASH streams / MP3 extraction.
    /// `false` trims the ladder to what plays without it (progressive MP4 +
    /// M4A audio) — the UI shows a hint instead of failing at download time.
    pub ffmpeg: bool,
}

/// Parse raw `yt-dlp -J` JSON into [`MediaProbe`].
pub fn distill(info: &serde_json::Value, has_ffmpeg: bool) -> MediaProbe {
    let title = info
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("video")
        .to_string();
    let is_live = info
        .get("is_live")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let qualities = quality_ladder_with(info, has_ffmpeg);
    let best = qualities
        .first()
        .map(|q| q.container.as_str())
        .unwrap_or("mp4");
    MediaProbe {
        kind: "media",
        extractor: info
            .get("extractor_key")
            .or_else(|| info.get("extractor"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        title: Some(title.clone()),
        duration: info.get("duration").and_then(|v| v.as_f64()),
        thumbnail: info
            .get("thumbnail")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        webpage_url: info
            .get("webpage_url")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        is_live,
        suggested_filename: format!("{}.{best}", title_to_filename(&title)),
        qualities,
        ffmpeg: has_ffmpeg,
    }
}

/// Turn a media title into a safe, readable filename stem.
/// `sanitize_filename` alone treats `/` as "last path segment wins", which
/// would gut titles like `Sample/Test` into `Test` — flatten separators to
/// dashes first so video titles survive intact.
pub fn title_to_filename(title: &str) -> String {
    let flat = title.replace(['/', '\\'], "-");
    let s = sanitize_filename(&flat);
    if s.is_empty() || s == "download.bin" {
        "media".into()
    } else {
        s
    }
}

/// Optional `--cookies-from-browser <name>` args from the
/// `ytdlp_cookies_browser` setting. YouTube (and friends) show
/// "confirm you're not a bot" walls to some IPs; piggy-backing the
/// user's signed-in browser cookies is the same trick IDM/FDM use.
/// Empty / unset setting → empty args (no cookies).
pub fn cookies_args_from_settings(db: &Arc<std::sync::Mutex<rusqlite::Connection>>) -> Vec<String> {
    let name = SettingsRepo::new(db)
        .get("ytdlp_cookies_browser")
        .ok()
        .flatten()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if name.is_empty() || name == "none" {
        return Vec::new();
    }
    // Only accept known browser names — this string is passed to a
    // subprocess; never forward arbitrary junk.
    const KNOWN: [&str; 9] = [
        "brave", "chrome", "chromium", "edge", "firefox", "opera", "safari", "vivaldi", "whale",
    ];
    if KNOWN.contains(&name.as_str()) {
        vec!["--cookies-from-browser".into(), name]
    } else {
        Vec::new()
    }
}

/// Run `yt-dlp -J --no-playlist --skip-download` and return the raw JSON.
pub async fn probe_url(
    binary: &Path,
    url: &str,
    extra_args: &[String],
) -> Result<serde_json::Value, MediaError> {
    let out = tokio::time::timeout(
        PROBE_TIMEOUT,
        tokio::process::Command::new(binary)
            .args([
                "-J",
                "--no-playlist",
                "--no-warnings",
                "--skip-download",
                "--socket-timeout",
                "15",
            ])
            .args(extra_args)
            .arg(url)
            .output(),
    )
    .await
    .map_err(|_| MediaError::Probe("probe timed out after 45s".into()))?
    .map_err(MediaError::Io)?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    if !out.status.success() || stdout.trim().is_empty() {
        let tail = stderr
            .lines()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" | ");
        return Err(MediaError::Probe(if tail.is_empty() {
            format!("yt-dlp exited with {:?}", out.status.code())
        } else {
            tail
        }));
    }
    serde_json::from_str(stdout.trim())
        .map_err(|e| MediaError::Probe(format!("unparseable yt-dlp JSON: {e}")))
}

fn fmt_size(f: &serde_json::Value, keys: &[&str]) -> Option<u64> {
    for k in keys {
        if let Some(n) = f.get(*k).and_then(|v| v.as_u64()) {
            if n > 0 {
                return Some(n);
            }
        }
    }
    None
}

fn fmt_num(f: &serde_json::Value, key: &str) -> Option<f64> {
    f.get(key).and_then(|v| v.as_f64())
}

fn codec_of(f: &serde_json::Value, key: &str) -> Option<String> {
    f.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| s != "none" && !s.is_empty())
}

fn is_video(f: &serde_json::Value) -> bool {
    codec_of(f, "vcodec").is_some()
}

fn is_audio(f: &serde_json::Value) -> bool {
    codec_of(f, "acodec").is_some()
}

fn is_progressive(f: &serde_json::Value) -> bool {
    is_video(f) && is_audio(f)
}

fn ext_of(f: &serde_json::Value) -> String {
    f.get("ext")
        .and_then(|v| v.as_str())
        .unwrap_or("mp4")
        .to_string()
}

/// Score a video format for the "one entry per height" ladder.
/// MP4/H.264 first (plays everywhere — the IDM/FDM default), then FPS,
/// then bitrate. Ordered ints avoid f64-Ord issues.
fn video_score(f: &serde_json::Value) -> (u8, u32, u32) {
    let ext = ext_of(f);
    let container_bonus = match ext.as_str() {
        "mp4" => 2u8,
        "webm" => 1,
        _ => 0,
    };
    let h264 = codec_of(f, "vcodec")
        .map(|c| c.starts_with("avc"))
        .unwrap_or(false) as u8;
    let fps = fmt_num(f, "fps").unwrap_or(0.0) as u32;
    let tbr = fmt_num(f, "tbr").unwrap_or(0.0) as u32;
    (container_bonus + h264, fps, tbr)
}

fn height_label(height: u32) -> (&'static str, Option<String>) {
    match height {
        4320 => ("4320p", Some("8K".into())),
        2160 => ("2160p", Some("4K".into())),
        1440 => ("1440p", Some("QHD".into())),
        1080 => ("1080p", Some("Full HD".into())),
        720 => ("720p", Some("HD".into())),
        480 => ("480p", Some("SD".into())),
        360 => ("360p", None),
        240 => ("240p", None),
        144 => ("144p", None),
        h => {
            // Static strings only for the common rungs; the rest format.
            (Box::leak(format!("{h}p").into_boxed_str()), None)
        }
    }
}

/// Build the de-duplicated quality ladder from raw yt-dlp formats.
///
/// Guarantees (the v0.6.1 contract, mirroring IDM/FDM):
/// * **one entry per resolution** — an mp4 1080p and a webm 1080p collapse
///   into a single `1080p` option (MP4/H.264 preferred);
/// * `best` first (highest resolution), audio-only last;
/// * DASH video+audio pairs are merged by yt-dlp via `A+B` selectors with
///   `--merge-output-format mp4`, so the user never sees mkv/webm
///   duplicates of the same quality.
pub fn quality_ladder(info: &serde_json::Value) -> Vec<QualityOption> {
    quality_ladder_with(info, true)
}

/// Without ffmpeg the ladder degrades gracefully: DASH downloads need a
/// merge (video-only + audio-only), so only progressive formats qualify;
/// audio-only becomes a plain M4A grab (no `-x` conversion). Everything
/// else would explode at download time with a cryptic yt-dlp error —
/// better to not offer what can't be delivered.
pub fn quality_ladder_with(info: &serde_json::Value, has_ffmpeg: bool) -> Vec<QualityOption> {
    let empty = Vec::new();
    let formats = info
        .get("formats")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);

    // ---- audio candidates (for merging sizes + the audio-only option) ----
    let mut best_audio_size: Option<u64> = None;
    let mut best_audio_id: Option<String> = None;
    let mut best_audio_score = 0.0f64;
    for f in formats {
        if !is_audio(f) || is_video(f) {
            continue;
        }
        let tbr = fmt_num(f, "tbr").unwrap_or(0.0);
        let abr = fmt_num(f, "abr").unwrap_or(0.0);
        let score = tbr.max(abr);
        if score > best_audio_score {
            best_audio_score = score;
            best_audio_id = f
                .get("format_id")
                .and_then(|v| v.as_str())
                .map(String::from);
            best_audio_size = fmt_size(f, &["filesize", "filesize_approx"]);
        }
    }

    // ---- group video formats by height ----
    let mut by_height: BTreeMap<u32, Vec<&serde_json::Value>> = BTreeMap::new();
    for f in formats {
        if !is_video(f) {
            continue;
        }
        if !has_ffmpeg && !is_progressive(f) {
            continue; // can't merge video-only without ffmpeg
        }
        let height = fmt_num(f, "height").unwrap_or(0.0) as u32;
        if height == 0 {
            continue; // audio-only or broken entries
        }
        by_height.entry(height).or_default().push(f);
    }

    let mut ladder: Vec<QualityOption> = Vec::new();
    // Descending by height: BTreeMap iterates ascending → rev.
    for (&height, group) in by_height.iter().rev() {
        // Pick ONE format per height: progressive MP4 if available (plays
        // anywhere, no merge needed), else the best video-only candidate.
        let progressive_mp4 = group
            .iter()
            .filter(|f| is_progressive(f) && ext_of(f) == "mp4")
            .max_by_key(|f| video_score(f));
        let chosen_progressive = progressive_mp4.copied();
        let chosen = chosen_progressive.unwrap_or_else(|| {
            group
                .iter()
                .copied()
                .max_by_key(|f| video_score(f))
                .unwrap_or(group[0])
        });

        let fid = chosen
            .get("format_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let ext = ext_of(chosen);
        let container = if chosen_progressive.is_some() {
            ext
        } else {
            "mp4".to_string() // merged via --merge-output-format mp4
        };

        let mut size = fmt_size(chosen, &["filesize", "filesize_approx"]);
        if chosen_progressive.is_none() {
            // Merged download: video + audio sizes.
            if let (Some(v), Some(a)) = (size, best_audio_size) {
                size = Some(v + a);
            }
        }

        let fps = fmt_num(chosen, "fps").unwrap_or(0.0) as u32;
        let (label, note) = height_label(height);
        let mut note = note;
        if fps >= 50 {
            note = Some(match note {
                Some(n) => format!("{n}, {fps}fps"),
                None => format!("{fps}fps"),
            });
        }
        // DASH video-only: explicit selector pairs with bestaudio.
        let id = if chosen_progressive.is_some() {
            fid.clone()
        } else {
            match &best_audio_id {
                Some(aid) => format!("{fid}+{aid}"),
                None => fid.clone(),
            }
        };

        ladder.push(QualityOption {
            id,
            label: label.to_string(),
            height: Some(height),
            container,
            size_bytes: size,
            note,
            audio_only: false,
        });
    }

    // ---- audio-only option (like IDM's "audio" mode) ----
    // With ffmpeg: extract MP3 (the format everyone expects). Without:
    // plain bestaudio M4A — no postprocessor, still plays everywhere.
    let (audio_label, audio_container, audio_id) = if has_ffmpeg {
        (
            "Audio only (MP3)",
            "mp3",
            best_audio_id.unwrap_or_else(|| "bestaudio".into()),
        )
    } else {
        (
            "Audio only (M4A)",
            "m4a",
            "bestaudio[ext=m4a]/bestaudio".to_string(),
        )
    };
    ladder.push(QualityOption {
        id: audio_id,
        label: audio_label.to_string(),
        height: None,
        container: audio_container.to_string(),
        size_bytes: best_audio_size,
        note: None,
        audio_only: true,
    });

    ladder
}

/// yt-dlp argv for a media download task (everything except the binary).
/// `extract_mp3` is only true when ffmpeg exists — the `-x` postprocessor
/// requires it; a plain M4A grab needs no postprocessing at all.
pub fn download_args(
    url: &str,
    selector: &str,
    extract_mp3: bool,
    out_template: &str,
    rate_limit_bps: Option<u64>,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--no-playlist".into(),
        "--no-warnings".into(),
        "--newline".into(),
        "--socket-timeout".into(),
        "15".into(),
        "-f".into(),
        selector.to_string(),
        "--progress-template".into(),
        format!("download:{PROGRESS_PREFIX}%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.speed)s|%(progress.eta)s"),
        "-o".into(),
        out_template.to_string(),
    ];
    if extract_mp3 {
        args.extend([
            "-x".into(),
            "--audio-format".into(),
            "mp3".into(),
            "--audio-quality".into(),
            "0".into(),
        ]);
    } else {
        args.extend(["--merge-output-format".into(), "mp4".into()]);
    }
    if let Some(bps) = rate_limit_bps {
        if bps > 0 {
            args.extend(["--limit-rate".into(), format!("{}k", bps / 1024)]);
        }
    }
    args.push(url.to_string());
    args
}

/// Parse one progress line (after the prefix). Returns
/// `(downloaded, total, speed_bps)` — `NA` fields become `None`.
pub fn parse_progress_line(payload: &str) -> Option<(i64, Option<i64>, u64)> {
    let mut it = payload.trim().split('|');
    let downloaded = it.next()?.trim();
    let total = it.next().unwrap_or("NA").trim();
    let speed = it.next().unwrap_or("NA").trim();
    let downloaded: i64 = downloaded.parse().ok()?;
    let total: Option<i64> = total.parse().ok();
    let speed: u64 = speed.parse::<f64>().map(|s| s as u64).unwrap_or(0);
    Some((downloaded, total, speed))
}

/// Effective media-download rate limit from settings (mirrors engine QoS).
fn rate_limit_from_settings(db: &Arc<std::sync::Mutex<rusqlite::Connection>>) -> Option<u64> {
    let repo = SettingsRepo::new(db);
    let enabled = repo.get("qos_enabled").ok().flatten().as_deref() == Some("true");
    if !enabled {
        return None;
    }
    repo.get("qos_target_bps")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u64>().ok())
}

// ---------------------------------------------------------------------------
// Media download coordinator (engine calls this for source = "media")
// ---------------------------------------------------------------------------

/// Mirror of the HTTP task coordinator, but driving a yt-dlp subprocess.
///
/// State machine is identical to `run_task_coordinator`:
/// downloading → (paused | complete | error | removed), with progress
/// persisted to the task row and `task:progress` events broadcast.
pub(crate) async fn run_ytdlp_coordinator(
    db: Arc<std::sync::Mutex<rusqlite::Connection>>,
    events: EventBus,
    task_id: String,
    mut cmd_rx: tokio::sync::mpsc::Receiver<crate::engine::TaskCommand>,
) -> Result<(), crate::engine::EngineError> {
    use crate::engine::EngineError;

    let row = {
        let repo = TasksRepo::new(&db);
        repo.get(&task_id)?
            .ok_or_else(|| EngineError::TaskNotFound(task_id.clone()))?
    };

    TasksRepo::new(&db).touch(&task_id, TaskState::Downloading, row.downloaded_bytes, None)?;
    events.emit(EngineEvent::task_state(
        &task_id,
        TaskState::Downloading,
        None,
    ));

    // Parse the media metadata the API stored at task creation.
    #[derive(Deserialize)]
    #[allow(dead_code)] // audio_only/container kept for observability parity
    struct Meta {
        selector: String,
        #[serde(default)]
        audio_only: bool,
        /// audio_only AND ffmpeg present — `-x --audio-format mp3` needs it.
        #[serde(default)]
        extract_mp3: bool,
        /// Promised container ("mp4" / "mp3" / "m4a") — display + filename.
        #[serde(default)]
        container: String,
    }
    let meta: Meta = match row
        .media_meta
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
    {
        Some(m) => m,
        None => {
            // Fail the ROW, not just the coordinator — a `?` here would
            // strand the task in `downloading` forever (v0.6.1 E2E catch).
            let msg = "media task is missing media_meta".to_string();
            TasksRepo::new(&db).touch(
                &task_id,
                TaskState::Error,
                row.downloaded_bytes,
                Some(&msg),
            )?;
            events.emit(EngineEvent::task_state(
                &task_id,
                TaskState::Error,
                Some(&msg),
            ));
            return Err(EngineError::Other(msg));
        }
    };

    // Binary: locate or auto-install (first-run UX: no manual setup).
    let binary = match ensure_ytdlp().await {
        Ok(p) => p,
        Err(e) => {
            let msg = e.to_string();
            TasksRepo::new(&db).touch(
                &task_id,
                TaskState::Error,
                row.downloaded_bytes,
                Some(&msg),
            )?;
            events.emit(EngineEvent::task_state(
                &task_id,
                TaskState::Error,
                Some(&msg),
            ));
            return Err(EngineError::Other(msg));
        }
    };

    // Output template: `<save_path minus extension>.%(ext)s` — the promised
    // filename is `<base>.mp4` (or `.mp3`), and yt-dlp fills the real ext.
    let save = PathBuf::from(&row.save_path);
    let parent = save
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let _ = std::fs::create_dir_all(&parent);
    let stem = save
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "media".into());
    let out_template = parent
        .join(format!("{stem}.%(ext)s"))
        .to_string_lossy()
        .into_owned();

    let mut args = download_args(
        &row.url,
        &meta.selector,
        meta.extract_mp3,
        &out_template,
        rate_limit_from_settings(&db),
    );
    args.extend(cookies_args_from_settings(&db));

    info!(task = %task_id, binary = %binary.display(), "yt-dlp coordinator starting");

    let mut child = match tokio::process::Command::new(&binary)
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("could not start yt-dlp: {e}");
            TasksRepo::new(&db).touch(
                &task_id,
                TaskState::Error,
                row.downloaded_bytes,
                Some(&msg),
            )?;
            events.emit(EngineEvent::task_state(
                &task_id,
                TaskState::Error,
                Some(&msg),
            ));
            return Err(EngineError::Io(e));
        }
    };

    let mut stdout = tokio::io::BufReader::new(child.stdout.take().expect("stdout piped"));
    let stderr_pipe = child.stderr.take().expect("stderr piped");

    // Collect stderr tail for the error message (bounded).
    let stderr_task = tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut reader = tokio::io::BufReader::new(stderr_pipe);
        let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if tail.len() >= 8 {
                        tail.pop_front();
                    }
                    tail.push_back(line.trim_end().to_string());
                }
            }
        }
        tail.into_iter().collect::<Vec<_>>().join(" | ")
    });

    let mut downloaded: i64 = 0;
    let mut total: Option<i64> = None;
    let mut last_emit = std::time::Instant::now();
    let mut speed_window_start = std::time::Instant::now();
    let mut speed_window_bytes = 0i64;
    let mut last_speed: u64 = 0;
    let mut line_buf = String::new();

    let outcome: Result<String, String> = loop {
        tokio::select! {
            biased;
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(crate::engine::TaskCommand::Pause) => {
                        info!(task = %task_id, "yt-dlp pause requested");
                        let _ = child.kill().await;
                        TasksRepo::new(&db).touch(&task_id, TaskState::Paused, downloaded, None)?;
                        events.emit(EngineEvent::task_state(&task_id, TaskState::Paused, None));
                        let _ = stderr_task.await;
                        return Ok(()); // paused — coordinator exits
                    }
                    Some(crate::engine::TaskCommand::Cancel) | None => {
                        info!(task = %task_id, "yt-dlp cancel requested");
                        let _ = child.kill().await;
                        TasksRepo::new(&db).touch(&task_id, TaskState::Removed, downloaded, None)?;
                        events.emit(EngineEvent::task_state(&task_id, TaskState::Removed, None));
                        let _ = stderr_task.await;
                        return Ok(());
                    }
                }
            }
            read = tokio::io::AsyncBufReadExt::read_line(&mut stdout, &mut line_buf) => {
                match read {
                    Ok(0) | Err(_) => break Ok(String::new()), // stream closed
                    Ok(_) => {
                        if let Some(payload) = line_buf.trim_end().strip_prefix(PROGRESS_PREFIX) {
                            if let Some((d, t, s)) = parse_progress_line(payload) {
                                downloaded = d;
                                if t.is_some() {
                                    total = t;
                                }
                                if s > 0 {
                                    last_speed = s;
                                }
                                if last_emit.elapsed() >= Duration::from_millis(500) {
                                    let elapsed = speed_window_start.elapsed().as_secs_f64();
                                    let win_speed = if elapsed > 0.0 {
                                        ((downloaded - speed_window_bytes).max(0) as f64 / elapsed) as u64
                                    } else {
                                        0
                                    };
                                    last_speed = if win_speed > 0 { win_speed } else { last_speed };
                                    let _ = TasksRepo::new(&db).touch(&task_id, TaskState::Downloading, downloaded, None);
                                    events.emit(EngineEvent::task_progress(&task_id, downloaded, total, last_speed));
                                    last_emit = std::time::Instant::now();
                                    speed_window_start = std::time::Instant::now();
                                    speed_window_bytes = downloaded;
                                }
                            }
                        }
                        line_buf.clear();
                    }
                }
            }
        }
    };

    // Give the child a moment to exit; kill_on_drop guards the pathological
    // hang — no unbounded wait here.
    let status = match tokio::time::timeout(Duration::from_secs(60), child.wait()).await {
        Ok(r) => r.map_err(EngineError::Io)?,
        Err(_) => {
            let msg = "yt-dlp did not exit after stream closed".to_string();
            TasksRepo::new(&db).touch(&task_id, TaskState::Error, downloaded, Some(&msg))?;
            events.emit(EngineEvent::task_state(
                &task_id,
                TaskState::Error,
                Some(&msg),
            ));
            return Err(EngineError::Other(msg));
        }
    };
    let stderr_tail = stderr_task.await.unwrap_or_default();

    if outcome.is_ok() && status.success() {
        // Find what actually landed: `<stem>.*` in the parent dir, skipping
        // .part/.ytdl leftovers. Fix up filename/save_path if the extension
        // differs from what we promised (rare — merge usually honors mp4).
        let final_path = find_output_file(&parent, &stem);
        let final_size = final_path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len() as i64);
        if let Some(p) = &final_path {
            let fname = p
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| row.filename.clone());
            if fname != row.filename || p.to_string_lossy() != row.save_path {
                let _ = TasksRepo::new(&db).rename(&task_id, &fname, &p.to_string_lossy());
            }
        }
        TasksRepo::new(&db).touch(
            &task_id,
            TaskState::Complete,
            final_size.unwrap_or(downloaded),
            None,
        )?;
        events.emit(EngineEvent::task_state(&task_id, TaskState::Complete, None));
        info!(task = %task_id, "yt-dlp download complete");
        return Ok(());
    }

    let msg = if stderr_tail.is_empty() {
        format!("yt-dlp exited with {:?}", status.code())
    } else {
        format!("yt-dlp: {stderr_tail}")
    };
    TasksRepo::new(&db).touch(&task_id, TaskState::Error, downloaded, Some(&msg))?;
    events.emit(EngineEvent::task_state(
        &task_id,
        TaskState::Error,
        Some(&msg),
    ));
    Err(EngineError::Other(msg))
}

/// Find the finished output file for an output template `<stem>.%(ext)s`:
/// the freshest `<stem>.<ext>` that is not a yt-dlp temp artifact.
fn find_output_file(parent: &Path, stem: &str) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let entries = std::fs::read_dir(parent).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(stem) {
            continue;
        }
        let rest = &name[stem.len()..];
        if !rest.starts_with('.') {
            continue;
        }
        let ext = &rest[1..];
        if ext.is_empty()
            || ext.ends_with(".part")
            || ext.ends_with(".ytdl")
            || ext.ends_with(".tmp")
        {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                if best.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
                    best = Some((mtime, entry.path()));
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

// ---------------------------------------------------------------------------
// Tests — the pure logic (ladder, parsing, filename) is the contract.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn yt_like() -> serde_json::Value {
        json!({
            "id": "dQw4w9WgXcQ",
            "title": "Sample Video — ពិសោធន៍/test",
            "extractor_key": "Youtube",
            "duration": 212.0,
            "thumbnail": "https://i.ytimg.com/vi/x/max.jpg",
            "webpage_url": "https://www.youtube.com/watch?v=x",
            "is_live": false,
            "formats": [
                { "format_id": "140", "ext": "m4a", "acodec": "mp4a.40.2", "vcodec": "none", "tbr": 129.0, "filesize": 3400000 },
                { "format_id": "251", "ext": "webm", "acodec": "opus", "vcodec": "none", "tbr": 160.0, "filesize": 4200000 },
                // progressive mp4 360p + 720p (play anywhere)
                { "format_id": "18", "ext": "mp4", "acodec": "mp4a.40.2", "vcodec": "avc1.42001E", "height": 360, "tbr": 600.0, "filesize": 15000000 },
                { "format_id": "22", "ext": "mp4", "acodec": "mp4a.40.2", "vcodec": "avc1.64001F", "height": 720, "tbr": 1200.0, "filesize": 30000000 },
                // DASH video-only: mp4 + webm duplicates at 1080p, 60fps variant
                { "format_id": "137", "ext": "mp4", "acodec": "none", "vcodec": "avc1.640028", "height": 1080, "fps": 30, "tbr": 4500.0, "filesize": 120000000 },
                { "format_id": "299", "ext": "mp4", "acodec": "none", "vcodec": "avc1.64002a", "height": 1080, "fps": 60, "tbr": 6000.0, "filesize": 160000000 },
                { "format_id": "248", "ext": "webm", "acodec": "none", "vcodec": "vp9", "height": 1080, "fps": 30, "tbr": 4000.0, "filesize": 110000000 },
                // 4K webm-only
                { "format_id": "313", "ext": "webm", "acodec": "none", "vcodec": "vp9", "height": 2160, "fps": 30, "tbr": 18000.0, "filesize": 480000000 }
            ]
        })
    }

    #[test]
    fn ladder_has_one_entry_per_height() {
        let q = quality_ladder(&yt_like());
        let mut heights: Vec<u32> = q.iter().filter_map(|o| o.height).collect();
        heights.sort();
        heights.dedup();
        assert_eq!(
            heights.len(),
            q.iter().filter(|o| o.height.is_some()).count(),
            "duplicate heights leaked into the ladder"
        );
        assert_eq!(heights, vec![360, 720, 1080, 2160]);
    }

    #[test]
    fn ladder_prefers_mp4_at_1080p_and_merges_audio() {
        let q = quality_ladder(&yt_like());
        let o1080 = q.iter().find(|o| o.height == Some(1080)).unwrap();
        // 60fps avc1 beats 30fps avc1 beats vp9 webm.
        assert!(o1080.id.starts_with("299+"), "got {}", o1080.id);
        assert_eq!(o1080.container, "mp4");
        assert_eq!(o1080.note.as_deref(), Some("Full HD, 60fps"));
        // size = video + audio
        assert_eq!(o1080.size_bytes, Some(160000000 + 4200000));
    }

    #[test]
    fn ladder_descending_then_audio_last() {
        let q = quality_ladder(&yt_like());
        assert!(q.first().unwrap().height.unwrap() == 2160);
        let last = q.last().unwrap();
        assert!(last.audio_only);
        assert_eq!(last.container, "mp3");
        assert_eq!(last.label, "Audio only (MP3)");
    }

    #[test]
    fn audio_only_uses_best_bitrate_audio() {
        let q = quality_ladder(&yt_like());
        let audio = q.last().unwrap();
        assert_eq!(audio.id, "251"); // 160kbps opus > 129kbps m4a
        assert_eq!(audio.size_bytes, Some(4200000));
    }

    #[test]
    fn progressive_720p_uses_plain_id() {
        let q = quality_ladder(&yt_like());
        let o720 = q.iter().find(|o| o.height == Some(720)).unwrap();
        assert_eq!(o720.id, "22"); // no merge needed
        assert_eq!(o720.size_bytes, Some(30000000));
    }

    #[test]
    fn progress_line_parses_na_fields() {
        assert_eq!(
            parse_progress_line("1234|5678|2048.5|12"),
            Some((1234, Some(5678), 2048))
        );
        assert_eq!(parse_progress_line("100|NA|NA|NA"), Some((100, None, 0)));
        assert_eq!(parse_progress_line("garbage"), None);
    }

    #[test]
    fn download_args_shape() {
        let args = download_args("https://x/y", "137+140", false, "/tmp/a.%(ext)s", None);
        assert!(args.contains(&"-f".to_string()));
        assert!(args.contains(&"137+140".to_string()));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--merge-output-format" && w[1] == "mp4"));
        assert!(args.last().unwrap() == "https://x/y");

        let audio = download_args(
            "https://x/y",
            "bestaudio",
            true,
            "/tmp/a.%(ext)s",
            Some(2 * 1024 * 1024),
        );
        assert!(audio
            .windows(2)
            .any(|w| w[0] == "--audio-format" && w[1] == "mp3"));
        assert!(audio
            .windows(2)
            .any(|w| w[0] == "--limit-rate" && w[1] == "2048k"));
    }

    #[test]
    fn distill_suggested_filename_is_sanitized() {
        let probe = distill(&yt_like(), true);
        let name = probe.suggested_filename;
        assert!(name.ends_with(".mp4"));
        assert!(!name.contains('/'), "slash survived sanitization: {name}");
        assert!(name.starts_with("Sample Video"), "got: {name}");
        assert!(name.contains("ពិសោធន៍-test"), "title mangled: {name}");
    }

    #[test]
    fn title_to_filename_flattens_separators() {
        assert_eq!(title_to_filename("AC/DC Live"), "AC-DC Live");
        assert_eq!(title_to_filename("a/b/c.mp4"), "a-b-c.mp4");
        assert_eq!(title_to_filename("???"), "media");
    }

    #[test]
    fn empty_formats_still_yield_audio_option() {
        let q = quality_ladder(&json!({ "formats": [] }));
        assert_eq!(q.len(), 1);
        assert!(q[0].audio_only);
    }

    #[test]
    fn without_ffmpeg_only_progressive_video_survives() {
        let q = quality_ladder_with(&yt_like(), false);
        // DASH-only heights (2160, 1080) vanish; progressive ones remain.
        let heights: Vec<u32> = q.iter().filter_map(|o| o.height).collect();
        assert_eq!(heights, vec![720, 360]);
        // All survivors are progressive (plain ids, no merge selector).
        for o in &q {
            if !o.audio_only {
                assert!(
                    !o.id.contains('+'),
                    "merge selector without ffmpeg: {}",
                    o.id
                );
            }
        }
    }

    #[test]
    fn without_ffmpeg_audio_is_m4a_no_conversion() {
        let q = quality_ladder_with(&yt_like(), false);
        let audio = q.last().unwrap();
        assert!(audio.audio_only);
        assert_eq!(audio.container, "m4a");
        assert_eq!(audio.label, "Audio only (M4A)");
        assert_eq!(audio.id, "bestaudio[ext=m4a]/bestaudio");
    }

    #[test]
    fn with_ffmpeg_audio_is_mp3() {
        let q = quality_ladder_with(&yt_like(), true);
        let audio = q.last().unwrap();
        assert_eq!(audio.container, "mp3");
        assert_eq!(audio.label, "Audio only (MP3)");
    }

    #[test]
    fn distill_reports_ffmpeg_flag() {
        assert!(distill(&yt_like(), true).ffmpeg);
        assert!(!distill(&yt_like(), false).ffmpeg);
        assert!(!distill(&yt_like(), false).qualities[0].audio_only);
    }
}
