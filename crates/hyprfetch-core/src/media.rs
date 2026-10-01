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

/// Floor for the managed yt-dlp binary. YouTube changes its player constantly
/// and old extractors silently degrade to a handful of formats with missing
/// sizes (the "only 360p" bug class). Versions compare lexicographically —
/// yt-dlp's `YYYY.MM.DD[.hhmmss]` format sorts correctly as a string, so this
/// stays dependency-free. Bump when cutting a HyprFetch release.
const YTDLP_MIN_VERSION: &str = "2026.01.01";

/// Parallel fragment downloads for DASH/HLS media. Every video-site stream
/// (YouTube included) is a bag of ~1–2 s fragments; downloading them one by
/// one is the "other apps are 10× faster" bug. 8 is the sweet spot IDM/FDM
/// also land on and is gentle enough for routers.
const CONCURRENT_FRAGMENTS: u32 = 8;

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
/// A managed binary that predates [`YTDLP_MIN_VERSION`] is refreshed from the
/// channel once per daemon run — stale extractors are the "YouTube shows only
/// one quality / no sizes" bug and users have no reason to ever notice.
pub async fn ensure_ytdlp() -> Result<PathBuf, MediaError> {
    static REFRESHED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if let Some(p) = find_ytdlp() {
        let is_managed = p == managed_bin_path();
        let first_call = REFRESHED.set(()).is_ok();
        if is_managed && first_call {
            let stale = version_of(&p)
                .await
                .ok()
                .and_then(|v| version_prefix(&v).map(|s| s.to_string()))
                .is_some_and(|s| s.as_str() < YTDLP_MIN_VERSION);
            if stale {
                warn!(
                    floor = YTDLP_MIN_VERSION,
                    "managed yt-dlp is stale — refreshing from the channel"
                );
                if install_ytdlp().await.is_ok() {
                    return Ok(managed_bin_path());
                }
                // Refresh failed — the stale binary still works; keep using it.
            }
        }
        return Ok(p);
    }
    install_ytdlp().await
}

/// First `YYYY.MM.DD` of a yt-dlp version string (nightly builds carry extra
/// components); `None` when the version does not start with a date at all.
fn version_prefix(v: &str) -> Option<&str> {
    let s = v.trim();
    if s.len() >= 10 && s.as_bytes().get(4) == Some(&b'.') {
        Some(&s[..10])
    } else {
        None
    }
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

// ---------------------------------------------------------------------------
// JS runtime (deno) — the YouTube speed/format unlock
// ---------------------------------------------------------------------------
// Modern yt-dlp needs a JavaScript runtime to solve YouTube's n-challenge and
// consent rounds. Without one it degrades exactly the way users report:
// few formats (sometimes only 360p), missing file sizes, and downloads
// throttled to a crawl. A managed deno next to the managed yt-dlp fixes all
// three; yt-dlp discovers it through the child process's PATH (see
// [`spawn_env`]) — no flags, so older yt-dlp builds keep working untouched.

/// Primary mirror of the official deno static Linux build.
pub const DENO_MIRROR: &str = "https://istias.tech/hyprfetch/updates/bin/deno/deno-linux-x86_64";

/// Fallback (GitHub, latest stable) — only used when the mirror fails.
pub const DENO_FALLBACK: &str =
    "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-unknown-linux-gnu.zip";

/// Locate `deno` on `$PATH`, then the managed copy.
pub fn find_deno() -> Option<PathBuf> {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("deno");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let managed = managed_bin_dir().join("deno");
    if managed.is_file() {
        return Some(managed);
    }
    None
}

/// Ensure a JS runtime exists for yt-dlp: use whatever is on PATH or already
/// managed; otherwise try to install the managed copy. Best-effort — a failed
/// install degrades to the old (slower, fewer formats) behaviour, never to a
/// broken download.
pub async fn ensure_deno() -> Option<PathBuf> {
    if let Some(p) = find_deno() {
        return Some(p);
    }
    match install_deno().await {
        Ok(p) => Some(p),
        Err(e) => {
            warn!("deno auto-install failed (media downloads stay functional but slower): {e}");
            None
        }
    }
}

/// Install the official deno static build into the managed bin dir.
/// The mirror serves the raw binary (no unzip needed); the GitHub fallback
/// ships a zip, extracted with whatever the host provides.
pub async fn install_deno() -> Result<PathBuf, MediaError> {
    let dest = managed_bin_dir().join("deno");
    std::fs::create_dir_all(managed_bin_dir())?;

    // 1. Self-hosted mirror: raw executable, same flow as yt-dlp.
    match download_binary(DENO_MIRROR, &dest).await {
        Ok(()) => {
            if let Ok(v) = version_of(&dest).await {
                info!(path = %dest.display(), version = %v, "deno installed from mirror");
                return Ok(dest);
            }
            let _ = std::fs::remove_file(&dest);
        }
        Err(e) => warn!("deno mirror download failed: {e}"),
    }

    // 2. GitHub fallback: zip archive → extract the `deno` binary.
    let zip = managed_bin_dir().join("deno.zip");
    download_binary(DENO_FALLBACK, &zip)
        .await
        .map_err(|e| MediaError::InstallFailed(format!("deno fallback download: {e}")))?;
    let extract = tokio::process::Command::new("python3")
        .args(["-m", "zipfile", "-e"])
        .arg(&zip)
        .arg(managed_bin_dir())
        .output()
        .await
        .map_err(MediaError::Io)?;
    let _ = std::fs::remove_file(&zip);
    if !extract.status.success() || !dest.is_file() {
        return Err(MediaError::InstallFailed(
            "deno fallback zip extraction failed".into(),
        ));
    }
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
    version_of(&dest).await?;
    info!(path = %dest.display(), "deno installed from GitHub fallback");
    Ok(dest)
}

/// Child-process environment for every yt-dlp invocation: the managed bin
/// dir is prepended to PATH so (a) a managed deno is discovered by yt-dlp's
/// JS-runtime lookup and (b) future managed tools need no extra plumbing.
fn spawn_env(cmd: &mut tokio::process::Command) {
    let managed = managed_bin_dir();
    let existing = std::env::var("PATH").unwrap_or_default();
    let path = if existing.is_empty() {
        managed.to_string_lossy().into_owned()
    } else {
        format!("{}:{existing}", managed.display())
    };
    cmd.env("PATH", path);
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
    /// `true` when [`QualityOption::size_bytes`] is a bitrate×duration
    /// estimate (YouTube often omits exact sizes). The UI renders it with a
    /// `~` so users never compare an estimate against a byte count.
    #[serde(default)]
    pub size_est: bool,
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
    let mut cmd = tokio::process::Command::new(binary);
    cmd.args([
        "-J",
        "--no-playlist",
        "--no-warnings",
        "--skip-download",
        "--socket-timeout",
        "15",
    ])
    .args(extra_args)
    .arg(url)
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped());
    spawn_env(&mut cmd);
    let out = tokio::time::timeout(PROBE_TIMEOUT, cmd.output())
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

/// Estimated byte size from the format's average bitrate and the media
/// duration — the honest answer for the YouTube formats that ship no
/// `filesize` at all (before: the UI just showed nothing, which users read
/// as "the extension can't get the correct size").
fn estimated_size(f: &serde_json::Value, duration: Option<f64>) -> Option<u64> {
    let tbr = fmt_num(f, "tbr")?; // kbit/s
    let dur = duration?; // seconds
    if tbr <= 0.0 || dur <= 0.0 {
        return None;
    }
    let bytes = (tbr * 1000.0 / 8.0 * dur) as u64;
    (bytes > 10_000).then_some(bytes)
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
    let duration = info.get("duration").and_then(|v| v.as_f64());

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
            best_audio_size = fmt_size(f, &["filesize", "filesize_approx"])
                .or_else(|| estimated_size(f, duration));
        }
    }
    // Does the chosen audio format ship an EXACT size (vs our estimate)?
    let best_audio_exact = formats.iter().any(|f| {
        f.get("format_id").and_then(|v| v.as_str()) == best_audio_id.as_deref()
            && fmt_size(f, &["filesize", "filesize_approx"]).is_some()
    });

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

        let reported = fmt_size(chosen, &["filesize", "filesize_approx"]);
        let (mut size, size_est) = match reported {
            Some(s) => (Some(s), false),
            None => match estimated_size(chosen, duration) {
                Some(s) => (Some(s), true),
                None => (None, false),
            },
        };
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
            size_est,
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
        size_est: best_audio_size.is_some() && !best_audio_exact,
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
        // DASH/HLS = thousands of small fragments; fetching them one at a
        // time is why video downloads crawled before v0.6.4. Ignored by
        // yt-dlp for non-fragmented formats.
        "--concurrent-fragments".into(),
        CONCURRENT_FRAGMENTS.to_string(),
        "-f".into(),
        selector.to_string(),
        // `total_bytes` is absent for fragmented downloads — yt-dlp only
        // provides `total_bytes_estimate` there, so BOTH are captured and
        // the coordinator uses whichever is present (v0.6.4: "?" totals).
        "--progress-template".into(),
        format!(
            "download:{PROGRESS_PREFIX}%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s"
        ),
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
/// `(downloaded, total, estimate, speed_bps)` — `NA` fields become `None`.
/// Fragmented downloads only report the estimate; direct formats only the
/// exact total. Callers fall back: `total.or(estimate)`.
pub fn parse_progress_line(payload: &str) -> Option<(i64, Option<i64>, Option<i64>, u64)> {
    let mut it = payload.trim().split('|');
    let downloaded = it.next()?.trim();
    let total = it.next().unwrap_or("NA").trim();
    let estimate = it.next().unwrap_or("NA").trim();
    let speed = it.next().unwrap_or("NA").trim();
    let downloaded: i64 = downloaded.parse().ok()?;
    let total: Option<i64> = total.parse().ok();
    let estimate: Option<i64> = estimate.parse().ok();
    let speed: u64 = speed.parse::<f64>().map(|s| s as u64).unwrap_or(0);
    Some((downloaded, total, estimate, speed))
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

/// Output files currently owned by a live yt-dlp coordinator (in-process).
/// Two yt-dlp processes writing the same output template race on the shared
/// `.part` / `.part-FragN.part` files and die with the cryptic
/// "Unable to rename file: [Errno 2]" — the task row users kept
/// screenshotting (v0.6.4: the second attempt is refused with a clear
/// message instead). The API layer rejects duplicates earlier; this is the
/// backstop for CLI/queued races.
static ACTIVE_OUTPUTS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

fn active_outputs() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    ACTIVE_OUTPUTS.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// Held for the lifetime of a coordinator; releases the claim on drop so
/// every early-return path (pause, cancel, error, success) unwinds cleanly.
struct OutputClaim {
    key: String,
}

impl Drop for OutputClaim {
    fn drop(&mut self) {
        active_outputs()
            .lock()
            .expect("output lock")
            .remove(&self.key);
    }
}

/// Try to claim `key`; `Err(())` when another coordinator owns it.
fn claim_output(key: &str) -> Result<OutputClaim, ()> {
    let mut set = active_outputs().lock().expect("output lock");
    set.insert(key.to_string())
        .then(|| OutputClaim {
            key: key.to_string(),
        })
        .ok_or(())
}

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
        /// Expected total size from the probe ladder — seeds the progress
        /// bar until yt-dlp reports one (fragmented downloads only emit an
        /// estimate well into the download).
        #[serde(default)]
        size: Option<u64>,
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

    // Refuse to run two yt-dlp processes on the same output (see
    // ACTIVE_OUTPUTS): they corrupt each other's fragment files.
    let _claim = match claim_output(&out_template) {
        Ok(c) => c,
        Err(()) => {
            let msg = format!(
                "another download is already writing to \"{stem}\" in this folder — wait for it or cancel it, then retry"
            );
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
            return Ok(()); // claimed by someone else — not an engine fault
        }
    };

    let mut args = download_args(
        &row.url,
        &meta.selector,
        meta.extract_mp3,
        &out_template,
        rate_limit_from_settings(&db),
    );
    args.extend(cookies_args_from_settings(&db));

    // Seed the task row with the expected size so the UI shows a real
    // total from the first second (fragmented downloads only report an
    // estimate later on).
    if let Some(s) = meta.size {
        let _ = TasksRepo::new(&db).set_total(&task_id, Some(s as i64));
    }

    info!(task = %task_id, binary = %binary.display(), "yt-dlp coordinator starting");

    let mut cmd = tokio::process::Command::new(&binary);
    cmd.args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    spawn_env(&mut cmd);
    let mut child = match cmd.spawn() {
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

    let mut downloaded: i64 = 0; // cumulative across video+audio phases
    let mut total: Option<i64> = meta.size.map(|s| s as i64); // seeded from the probe
    let mut phase_base: i64 = 0; // bytes finished by previous formats
    let mut phase_downloaded: i64 = 0; // bytes of the current format
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
                            if let Some((d, t, est, s)) = parse_progress_line(payload) {
                                // yt-dlp restarts its byte counter for every
                                // format (video, then audio, then merge). A
                                // collapse of >1 MiB means the next phase
                                // started — fold the finished phase into the
                                // base so the task bar never runs backwards.
                                if d + 1024 * 1024 < phase_downloaded {
                                    phase_base += phase_downloaded;
                                }
                                phase_downloaded = d;
                                downloaded = phase_base + d;
                                if let Some(t) = t.or(est) {
                                    let t = phase_base + t;
                                    // Estimates wobble early on — only grow.
                                    if t > total.unwrap_or(0) {
                                        total = Some(t);
                                        let _ = TasksRepo::new(&db).set_total(&task_id, total);
                                    }
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
        if let Some(sz) = final_size {
            let _ = TasksRepo::new(&db).set_total(&task_id, Some(sz));
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
            parse_progress_line("1234|5678|NA|2048.5|12"),
            Some((1234, Some(5678), None, 2048))
        );
        // Fragmented downloads: no exact total, estimate present.
        assert_eq!(
            parse_progress_line("100|NA|9000|NA|NA"),
            Some((100, None, Some(9000), 0))
        );
        assert_eq!(
            parse_progress_line("100|NA|NA|NA|NA"),
            Some((100, None, None, 0))
        );
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
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--concurrent-fragments" && w[1] == "8"));
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
    fn estimated_size_from_bitrate_and_duration() {
        // 4500 kbit/s × 10 s = 5.625 MB (the user's 10 s test video class).
        let f = json!({ "tbr": 4500.0 });
        assert_eq!(estimated_size(&f, Some(10.0)), Some(5_625_000));
        assert_eq!(estimated_size(&json!({ "tbr": 0 }), Some(10.0)), None);
        assert_eq!(estimated_size(&json!({ "tbr": 4500.0 }), None), None);
    }

    #[test]
    fn ladder_estimates_sizes_when_filesize_missing() {
        let mut info = yt_like().clone();
        // Strip every filesize: the "YouTube shows no sizes" regression.
        for f in info.get_mut("formats").unwrap().as_array_mut().unwrap() {
            f.as_object_mut().unwrap().remove("filesize");
        }
        info.as_object_mut()
            .unwrap()
            .insert("duration".into(), json!(212.0));
        let q = quality_ladder(&info);
        let o1080 = q.iter().find(|o| o.height == Some(1080)).unwrap();
        assert!(o1080.size_est, "1080p size should be marked estimated");
        assert!(o1080.size_bytes.unwrap() > 100_000_000);
        let audio = q.last().unwrap();
        assert!(audio.size_est);
        assert!(audio.size_bytes.unwrap() > 1_000_000);
    }

    #[test]
    fn output_claim_blocks_second_owner_then_releases() {
        let key = format!("/tmp/hf-test-claim-{}", std::process::id());
        {
            let c1 = claim_output(&key).expect("first claim must win");
            assert!(claim_output(&key).is_err(), "second claim must be refused");
            drop(c1);
        }
        let c2 = claim_output(&key).expect("claim after release must win");
        drop(c2);
        let _ = claim_output(&key).expect("claim after drop is reusable");
    }

    #[test]
    fn version_prefix_parses_dates_only() {
        assert_eq!(version_prefix("2026.08.19"), Some("2026.08.19"));
        assert_eq!(version_prefix("2026.08.19.232815"), Some("2026.08.19"));
        assert_eq!(version_prefix("abc"), None);
        assert_eq!(version_prefix("2025"), None);
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
