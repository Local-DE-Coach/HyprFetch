//! Self-update via the project's own update channel + sha256-verified
//! binary swap.
//!
//! The ONLY update source is the self-hosted channel: a plain HTTPS
//! manifest (`latest.json`) served from
//! `https://istias.tech/hyprfetch/updates/` and mirrored to by CI on
//! every release. No GitHub API, no rate limits, no tokens — and it
//! keeps working when the source repo is private. A check is one fast
//! GET; an install is download → sha256-verify (manifest hash) →
//! extract → atomic swap → (daemon restart, handled by the caller).
//!
//! The channel base is overridable (`HYPRFETCH_UPDATE_CHANNEL` /
//! `[update] channel` / `--channel`) so tests can run against local mock
//! servers; setting it to the empty string disables the updater
//! entirely.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Default self-hosted update channel (release mirror). CI uploads every
/// release's tarballs + a `latest.json` manifest here, so `hyprfetch update
/// --check` is one fast HTTPS GET with no GitHub involvement at all.
pub const DEFAULT_CHANNEL_URL: &str = "https://istias.tech/hyprfetch/updates/";
/// Human-facing page that documents the channel, install and update steps.
pub const UPDATES_PAGE_URL: &str = "https://istias.tech/hyprfetch/updates";
/// Timeout for the one small `latest.json` GET (fail fast, retry once).
const MANIFEST_TIMEOUT: Duration = Duration::from_secs(20);
/// TCP connect timeout for every update HTTP call.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// NO-BYTES ceiling for the archive download: if the server sends nothing
/// for this long the attempt is treated as stalled and RETRIED (with
/// resume). There is deliberately NO total download deadline — v0.4.6
/// wrapped the whole asset in a 60s timeout, so any download slower than
/// ~65 KB/s (or one stalled moment) died with the cryptic reqwest
/// "error decoding response body" the owner hit after 1m04s.
const IDLE_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Download attempts (the last two resume from where the bytes stopped).
const DOWNLOAD_ATTEMPTS: u32 = 3;
/// Root-owned privileged-update helper + its sudoers drop-in. Installed
/// ONCE with the user's password (the v0.4.7 one-click update setup);
/// afterwards in-app updates swap a system binary WITHOUT ever asking for
/// a password again — the same trick GUI package managers use (polkit
/// rules), just implemented with a narrow sudoers allowlist so it also
/// works on window managers with no polkit agent running.
pub const PRIV_HELPER_PATH: &str = "/usr/lib/hyprfetch/privileged-update";
pub const SUDOERS_PATH: &str = "/etc/sudoers.d/hyprfetch-update";

/// Errors returned by the updater.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("asset {0} not found in release")]
    AssetMissing(String),
    #[error("sha256 mismatch: expected {expected}, got {got}")]
    ChecksumMismatch { expected: String, got: String },
    #[error("archive does not contain a `hyprfetch` binary")]
    BinaryMissing,
    #[error("cannot determine current executable path")]
    ExePath,
    #[error("update channel: {0}")]
    Channel(String),
    #[error("cannot replace {path} — it lives in a system location (permission denied). {hint}")]
    RootNeeded { path: String, hint: String },
    /// The download is done and staged, but the system-owned install needs
    /// the user's password — every passwordless route (one-click helper,
    /// `sudo -n`, `pkexec`) was declined or unavailable. The staged binary
    /// is KEPT at `staged` so the WebUI's one-click setup (terminal window
    /// + one password entry) can finish the swap without re-downloading.
    #[error(
        "password required: {target} lives in a system location and no passwordless route answered"
    )]
    PasswordRequired {
        /// Staged new binary (kept until the swap completes).
        staged: String,
        /// System path to replace.
        target: String,
        /// Human hint shown by the CLI/UI.
        hint: String,
    },
    #[error("{0}")]
    Other(String),
}

/// What the updater may do when the running binary sits in a root-owned
/// directory (e.g. `/usr/bin` when installed via pacman/makepkg or .deb/.rpm).
///
/// A plain user process cannot write there, so the atomic swap would fail
/// with "permission denied". With [`Escalation::Auto`] the updater performs
/// the swap through a privilege tool (`sudo`, falling back to `doas`); with
/// [`Escalation::Refuse`] it returns [`UpdateError::RootNeeded`] with an
/// actionable hint instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escalation {
    /// Replace system-owned binaries via `sudo`/`doas` (sudo may prompt for a
    /// password — this is the interactive CLI choice).
    Auto,
    /// Escalate WITHOUT a terminal, so a daemon/WebUI can self-update a
    /// system install: passwordless `sudo -n` first, then `pkexec` (the
    /// polkit agent shows the GUI password prompt on desktops). Never
    /// blocks on a TTY password read.
    NonInteractive,
    /// Never escalate; fail with [`UpdateError::RootNeeded`] instead.
    Refuse,
}

/// A resolved privilege-escalation command: the program plus the argument
/// prefix that makes it non-interactive where applicable (`sudo -n`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivCmd {
    pub program: String,
    pub pre_args: Vec<&'static str>,
}

impl PrivCmd {
    /// Interactive `sudo`/`doas` (may prompt for a password) — CLI choice.
    pub fn interactive() -> Option<Self> {
        find_priv_tool().map(|program| Self {
            program: program.to_string(),
            pre_args: Vec::new(),
        })
    }

    /// Spawn `/bin/sh -c script` under this privilege command.
    fn run_sh(&self, script: &str) -> std::io::Result<std::process::ExitStatus> {
        let mut cmd = std::process::Command::new(&self.program);
        cmd.args(&self.pre_args);
        // pkexec requires the program as an absolute path (polkit rule);
        // `sudo`/`doas` resolve `sh` via their own secure_path anyway.
        cmd.arg("/bin/sh").arg("-c").arg(script);
        cmd.status()
    }
}

/// Non-interactive privilege escalation for daemons / the WebUI.
///
/// 1. passwordless `sudo` (`sudo -n true` succeeds — NOPASSWD entry or
///    cached credentials),
/// 2. `pkexec` — on a desktop session the polkit agent pops the graphical
///    password prompt, which is exactly how GUI package managers elevate.
///
/// Returns `None` when neither can run without a TTY; the caller then fails
/// with [`UpdateError::RootNeeded`] and an actionable hint.
pub fn find_priv_tool_noninteractive() -> Option<PrivCmd> {
    #[cfg(unix)]
    {
        if which_on_path("sudo") && sudo_n_ok() {
            return Some(PrivCmd {
                program: "sudo".to_string(),
                pre_args: vec!["-n"],
            });
        }
        if which_on_path("pkexec") {
            return Some(PrivCmd {
                program: "pkexec".to_string(),
                pre_args: Vec::new(),
            });
        }
        None
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// `sudo -n true` — succeeds only when sudo can run WITHOUT asking for a
/// password (NOPASSWD or cached timestamp), which is all a daemon can use.
#[cfg(unix)]
fn sudo_n_ok() -> bool {
    std::process::Command::new("sudo")
        .arg("-n")
        .arg("true")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Is `name` an executable file on `PATH`?
#[cfg(unix)]
fn which_on_path(name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        dir.join(name)
            .metadata()
            .map(|md| md.is_file() && md.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    })
}

/// Where to check for updates.
#[derive(Debug, Clone)]
pub struct UpdateConfig {
    /// Self-hosted update-channel base URL (`latest.json` lives at
    /// `<channel_url>/latest.json`). Empty string disables the updater
    /// (prints a pointer to [`UPDATES_PAGE_URL`] instead).
    pub channel_url: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            channel_url: DEFAULT_CHANNEL_URL.to_string(),
        }
    }
}

impl UpdateConfig {
    /// Client for the small manifest GET (bounded by a total timeout).
    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(MANIFEST_TIMEOUT)
            .user_agent(concat!("hyprfetch/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client")
    }

    /// Client for the ARCHIVE download: bounded connect, but NO total
    /// timeout — per-chunk stall detection is applied by [`download_asset`]
    /// so a slow connection takes as long as it takes while a stalled one
    /// is retried with resume.
    fn download_client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("hyprfetch/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client")
    }

    /// The active channel base (`None` when the updater is disabled via an
    /// empty `[update] channel` / env).
    pub fn effective_channel(&self) -> Option<&str> {
        let s = self.channel_url.trim();
        (!s.is_empty()).then_some(s)
    }
}

/// Metadata about the matching release asset.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AssetInfo {
    pub name: String,
    pub size: u64,
}

/// Result of an update check.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateCheck {
    /// Version this binary was built as (workspace version).
    pub current: String,
    /// Latest published version (tag minus `v` prefix).
    pub latest: String,
    /// True when `latest` is newer than `current`.
    pub available: bool,
    pub published_at: Option<String>,
    pub release_url: Option<String>,
    /// The archive matching this machine's target, when present.
    pub asset: Option<AssetInfo>,
    /// The channel base URL that answered.
    #[serde(default)]
    pub channel: Option<String>,
}

/// Compare dotted numeric versions (`0.3.1` > `0.3.0`); each chunk uses its
/// LEADING DIGITS as the numeric value (`7-test` → 7) and falls back to a
/// lexicographic tie-break — `0.4.7-beta` must compare as 7, not as 0.
pub fn version_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<(u64, String)> {
        v.trim()
            .trim_start_matches('v')
            .split('.')
            .map(|c| {
                let digits: String = c.chars().take_while(|ch| ch.is_ascii_digit()).collect();
                (digits.parse::<u64>().unwrap_or(0), c.to_string())
            })
            .collect()
    };
    let (a, b) = (parse(candidate), parse(current));
    for i in 0..a.len().max(b.len()) {
        let (ca, cb) = (
            a.get(i).cloned().unwrap_or((0, String::new())),
            b.get(i).cloned().unwrap_or((0, String::new())),
        );
        if ca != cb {
            return ca > cb;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Update channel — the one and only update source
//
// CI uploads every release's archives into `<channel>/<version>/` and
// writes `<channel>/latest.json`. One HTTPS GET answers "is there a new
// version?" with zero GitHub involvement: no rate limits, no tokens, and
// it works even when the source repo is private, because the mirror is
// populated by CI with deploy credentials, not by the client.
//
// Manifest schema (`latest.json`):
// {
//   "version": "0.4.0",
//   "tag": "v0.4.0",
//   "published_at": "2026-09-29T12:00:00Z",
//   "notes_url": "https://istias.tech/hyprfetch/updates",
//   "assets": {
//     "x86_64-unknown-linux-gnu":  {"url": "...", "sha256": "...", "size": 123},
//     "aarch64-unknown-linux-gnu": {"url": "...", "sha256": "...", "size": 123},
//     "x86_64-unknown-linux-musl": {"url": "...", "sha256": "...", "size": 123}
//   }
// }
// ---------------------------------------------------------------------------

/// One downloadable archive in the update-channel manifest.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ChannelAsset {
    /// Absolute download URL (usually `<channel>/<version>/<archive>.tar.gz`).
    pub url: String,
    /// sha256 of the archive — verified before anything is swapped.
    pub sha256: String,
    /// Archive size in bytes (informational; the hash is authoritative).
    #[serde(default)]
    pub size: u64,
}

/// The `latest.json` manifest served by the update channel.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ChannelManifest {
    /// Newest version (`0.4.0`, no `v` prefix).
    pub version: String,
    /// Raw tag (`v0.4.0`).
    pub tag: String,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub notes_url: Option<String>,
    /// Per-target-triple archives.
    pub assets: std::collections::BTreeMap<String, ChannelAsset>,
}

/// `latest.json` location for a channel base URL.
pub fn manifest_url(channel_base: &str) -> String {
    format!("{}/latest.json", channel_base.trim_end_matches('/'))
}

/// The manifest asset matching this machine's target triple.
pub fn pick_channel_asset(
    assets: &std::collections::BTreeMap<String, ChannelAsset>,
) -> Option<(String, ChannelAsset)> {
    target_candidates()
        .into_iter()
        .find_map(|t| assets.get(&t).cloned().map(|a| (t, a)))
}

/// Blocking: fetch + parse the channel manifest. `Err` on HTTP trouble or
/// a malformed manifest.
pub async fn fetch_manifest(cfg: &UpdateConfig) -> Result<ChannelManifest, UpdateError> {
    let base = cfg
        .effective_channel()
        .ok_or_else(|| UpdateError::Channel("updater disabled (empty channel url)".into()))?;
    let resp = cfg
        .client()
        .get(manifest_url(base))
        .send()
        .await
        .map_err(|e| UpdateError::Channel(format!("manifest get: {e}")))?;
    if !resp.status().is_success() {
        return Err(UpdateError::Channel(format!(
            "manifest get: http {}",
            resp.status()
        )));
    }
    let manifest: ChannelManifest = resp
        .json()
        .await
        .map_err(|e| UpdateError::Channel(format!("manifest parse: {e}")))?;
    Ok(manifest)
}

/// Check the self-hosted update channel. `Err` when the channel cannot be
/// reached (the caller surfaces [`UPDATES_PAGE_URL`] — there is no other
/// source to fall back to).
pub async fn check(cfg: &UpdateConfig) -> Result<UpdateCheck, UpdateError> {
    let base = cfg
        .effective_channel()
        .ok_or_else(|| UpdateError::Channel("updater disabled (empty channel url)".into()))?
        .to_string();
    let m = fetch_manifest(cfg).await?;
    let current = env!("CARGO_PKG_VERSION").to_string();
    let asset = pick_channel_asset(&m.assets).map(|(target, a)| AssetInfo {
        name: a
            .url
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(&format!("hyprfetch-{}-{target}.tar.gz", m.version))
            .to_string(),
        size: a.size,
    });
    Ok(UpdateCheck {
        available: version_newer(&m.version, &current),
        current,
        latest: m.version,
        published_at: m.published_at,
        release_url: m.notes_url,
        asset,
        channel: Some(base),
    })
}

/// Retry/idle limits for [`download_asset_limits`] (tests use short values).
#[derive(Debug, Clone, Copy)]
pub struct DownloadLimits {
    /// No-bytes ceiling per attempt.
    pub idle_timeout: Duration,
    /// Total attempts (the retries resume from the partial file).
    pub attempts: u32,
}

impl Default for DownloadLimits {
    fn default() -> Self {
        Self {
            idle_timeout: IDLE_READ_TIMEOUT,
            attempts: DOWNLOAD_ATTEMPTS,
        }
    }
}

/// Blocking-ish (async): STREAM the archive at `url` to `dest` with no
/// total deadline, resuming through stalls, and verify its sha256.
///
/// Why this exists: v0.4.6 buffered the whole asset behind one 60-second
/// total timeout, so any download slower than ~65 KB/s — or a single
/// stalled moment on the network — failed with the cryptic reqwest
/// "error decoding response body" (the owner's exact 1m04s failure).
/// This version:
///   • has NO total timeout — a slow download takes as long as it takes;
///   • aborts an attempt only when NO bytes arrive for 30s;
///   • retries up to [`DOWNLOAD_ATTEMPTS`] times, each attempt RESUMING
///     from where the previous one stopped (`Range: bytes=N-`);
///   • verifies sha256 at the end; a mismatch wipes the partial file and
///     retries fresh before surfacing [`UpdateError::ChecksumMismatch`].
///
/// Returns the sha256 (lowercase hex) of the verified file.
pub async fn download_asset(
    cfg: &UpdateConfig,
    url: &str,
    dest: &Path,
    expected_sha: String,
    expected_size: u64,
    progress: &mut (dyn FnMut(u64, u64) + Send),
) -> Result<String, UpdateError> {
    download_asset_limits(
        cfg,
        url,
        dest,
        expected_sha,
        expected_size,
        progress,
        DownloadLimits::default(),
    )
    .await
}

/// [`download_asset`] with explicit limits (tests use a short idle timeout
/// and fewer attempts so stall/retry paths run in milliseconds).
pub async fn download_asset_limits(
    cfg: &UpdateConfig,
    url: &str,
    dest: &Path,
    expected_sha: String,
    expected_size: u64,
    progress: &mut (dyn FnMut(u64, u64) + Send),
    limits: DownloadLimits,
) -> Result<String, UpdateError> {
    let DownloadLimits {
        idle_timeout,
        attempts,
    } = limits;
    let client = cfg.download_client();
    let mut last_network_err: Option<String> = None;

    for attempt in 1..=attempts {
        if attempt > 1 {
            tokio::time::sleep(Duration::from_secs(2 * attempt as u64 - 2)).await;
        }
        let partial = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);

        let mut req = client.get(url);
        if partial > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={partial}-"));
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(attempt, error = %e, "asset download attempt failed");
                last_network_err = Some(format!("asset get: {e}"));
                continue; // retry (resumes from the partial file)
            }
        };
        let status = resp.status();

        // Range Not Satisfiable → the partial file already covers the whole
        // asset (server confirms the range is past the end).
        if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            if expected_size > 0 && partial >= expected_size {
                return verify_sha(dest, &expected_sha);
            }
            let _ = std::fs::remove_file(dest); // hopeless partial — restart
            last_network_err = Some("asset get: http 416 with an incomplete partial".into());
            continue;
        }
        if !status.is_success() {
            // 5xx may heal; 4xx will not (except the 416 handled above).
            if status.is_server_error() && attempt < attempts {
                last_network_err = Some(format!("asset get: http {status}"));
                continue;
            }
            return Err(UpdateError::Channel(format!("asset get: http {status}")));
        }

        // 206 = resuming from `partial`; 200 = server ignored Range → restart.
        let resume_from = if status == reqwest::StatusCode::PARTIAL_CONTENT {
            partial
        } else {
            0
        };
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(dest)
        {
            Ok(f) => f,
            Err(e) => return Err(e.into()),
        };
        if let Err(e) = file.set_len(resume_from) {
            return Err(e.into());
        }
        use std::io::{Seek, Write as _};
        if let Err(e) = file.seek(std::io::SeekFrom::Start(resume_from)) {
            return Err(e.into());
        }

        let total_hint = if expected_size > 0 {
            expected_size.max(resume_from)
        } else {
            0
        };
        let mut received = resume_from;
        let mut stream = resp;
        let mut failure: Option<String> = None;
        loop {
            // Per-chunk stall detection: no bytes for the idle timeout
            // means the attempt died — the next one resumes.
            let chunk = match tokio::time::timeout(idle_timeout, stream.chunk()).await {
                Ok(Ok(Some(bytes))) => bytes,
                Ok(Ok(None)) => break, // clean EOF
                Ok(Err(e)) => {
                    failure = Some(format!("asset read: {e}"));
                    break;
                }
                Err(_) => {
                    failure = Some(format!(
                        "asset read: stalled (no bytes for {}s)",
                        idle_timeout.as_secs()
                    ));
                    break;
                }
            };
            if let Err(e) = file.write_all(&chunk) {
                return Err(UpdateError::Io(e));
            }
            received += chunk.len() as u64;
            progress(received, total_hint);
        }
        let _ = file.sync_all();

        if let Some(err) = failure {
            tracing::debug!(attempt, error = %err, "asset stream interrupted — will resume");
            last_network_err = Some(err);
            continue; // retry + resume
        }

        // Stream finished — the hash is the authority.
        match verify_sha(dest, &expected_sha) {
            Ok(sha) => return Ok(sha),
            Err(UpdateError::ChecksumMismatch { expected, got }) => {
                // A corrupt body must NEVER reach the swap: wipe the partial
                // and try again fresh; surface the mismatch only after the
                // final attempt.
                let _ = std::fs::remove_file(dest);
                if attempt < attempts {
                    tracing::debug!(attempt, "asset checksum mismatch — fresh retry");
                    last_network_err = Some(format!(
                        "asset checksum mismatch (expected {expected}, got {got})"
                    ));
                    continue;
                }
                return Err(UpdateError::ChecksumMismatch { expected, got });
            }
            Err(e) => return Err(e),
        }
    }

    Err(UpdateError::Channel(last_network_err.unwrap_or_else(
        || format!("asset download failed after {attempts} attempts"),
    )))
}

/// sha256 of `dest` compared against `expected`; returns the hash on match.
fn verify_sha(dest: &Path, expected: &str) -> Result<String, UpdateError> {
    let got = file_sha256(dest)?;
    if got == expected {
        Ok(got)
    } else {
        Err(UpdateError::ChecksumMismatch {
            expected: expected.to_string(),
            got,
        })
    }
}

/// Streaming sha256 of a file (constant memory, whatever the size).
fn file_sha256(path: &Path) -> Result<String, UpdateError> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// Install: fetch the manifest, download the archive for this target,
/// sha256-verify against the manifest, extract + atomic swap.
///
/// `escalation` decides how a system-owned install location is handled (see
/// [`Escalation`]). The download always succeeds for ANY user (it stages in
/// a temp dir); only the final swap can need privileges. For
/// [`Escalation::NonInteractive`] the swap walks a ladder — one-click
/// helper (silent) → `sudo -n` (silent) → `pkexec` (GUI prompt when a
/// polkit agent runs) → [`UpdateError::PasswordRequired`] with the staged
/// binary KEPT so the one-time setup can finish without re-downloading.
pub async fn apply(
    cfg: &UpdateConfig,
    chk: &UpdateCheck,
    escalation: Escalation,
) -> Result<ApplyResult, UpdateError> {
    apply_with_progress(cfg, chk, escalation, |_, _| {}).await
}

/// [`apply`] with a download-progress callback (`bytes_done`, `bytes_total`
/// — total is 0 when the server sends no length).
pub async fn apply_with_progress(
    cfg: &UpdateConfig,
    chk: &UpdateCheck,
    escalation: Escalation,
    mut progress: impl FnMut(u64, u64) + Send,
) -> Result<ApplyResult, UpdateError> {
    // Resolve the running binary up front (fail fast before the download).
    let exe = std::env::current_exe().map_err(|_| UpdateError::ExePath)?;
    if escalation == Escalation::Refuse && !can_swap_in_place(&exe) {
        return Err(root_needed(&exe, false));
    }

    let m = fetch_manifest(cfg).await?;
    let (_, asset) = pick_channel_asset(&m.assets)
        .ok_or_else(|| UpdateError::AssetMissing("target tarball (update channel)".into()))?;

    // Download → stage (works for ANY user; no privileges needed here).
    // `verified_sha` is the hash the downloader MEASURED (not just the
    // manifest's claim) — it goes into the result verbatim.
    let stage_dir = staging_dir();
    std::fs::create_dir_all(&stage_dir)?;
    let tarball = stage_dir.join("asset.tar.gz");
    let verified_sha = download_asset(
        cfg,
        &asset.url,
        &tarball,
        asset.sha256.trim().to_ascii_lowercase(),
        asset.size,
        &mut progress,
    )
    .await
    .inspect_err(|_| {
        let _ = std::fs::remove_dir_all(&stage_dir);
    })?;

    let new_bytes = extract_binary_file(&tarball)?;
    let staged = stage_dir.join("hyprfetch");
    let staged_result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&staged)?;
        f.write_all(&new_bytes)?;
        f.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    })();
    if let Err(e) = staged_result {
        let _ = std::fs::remove_dir_all(&stage_dir);
        return Err(e.into());
    }
    let _ = std::fs::remove_file(&tarball);

    // Swap — direct when the location is writable, otherwise (v0.4.9)
    // passwordless in-place routes first, then MIGRATE to ~/.local/bin.
    // Package-managed installs keep the classic privilege ladder. Every
    // path below is self-recovering: a failed rung leaves the installed
    // binary untouched.
    let (escalated, migration, system_fix) = if can_swap_in_place(&exe) {
        swap_binary_from_staged(&exe, &staged)?;
        let _ = std::fs::remove_dir_all(&stage_dir);
        (false, None, None)
    } else {
        match swap_or_migrate(&exe, &staged, escalation) {
            Ok(SwapOutcome::Escalated) => {
                let _ = std::fs::remove_dir_all(&stage_dir);
                (true, None, None)
            }
            Ok(SwapOutcome::Migrated(m, fix)) => {
                let _ = std::fs::remove_dir_all(&stage_dir);
                (false, Some(m), Some(fix))
            }
            Err(UpdateError::PasswordRequired {
                staged,
                target,
                hint,
            }) => {
                // KEEP the staging dir: the one-time setup (or a manual
                // `sudo hyprfetch update`) can finish the swap from it.
                // (Only package-managed installs can still hit this.)
                return Err(UpdateError::PasswordRequired {
                    staged,
                    target,
                    hint,
                });
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&stage_dir);
                return Err(e);
            }
        }
    };

    Ok(ApplyResult {
        current: chk.current.clone(),
        installed: chk.latest.clone(),
        // In-place/escalated swaps keep the rollback at `<exe>.old`; a
        // migration rolls back at `~/.local/bin/hyprfetch.old` (the system
        // dir was never touched, so no `.old` exists there).
        backup_path: match &migration {
            Some(m) => m
                .backup_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            None => Some(exe.with_extension("old").to_string_lossy().into_owned()),
        },
        sha256: verified_sha,
        escalated,
        migrated: migration.is_some(),
        new_path: migration
            .as_ref()
            .map(|m| m.new_path.to_string_lossy().into_owned()),
        path_fixes: migration.map(|m| m.path_fixes).unwrap_or_default(),
        system_fix_hint: system_fix.as_ref().and_then(SystemCopyFix::hint),
    })
}

/// A unique user-writable staging directory for one update attempt.
fn staging_dir() -> PathBuf {
    sweep_stale_staging();
    std::env::temp_dir().join(format!(
        "hyprfetch-update-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    ))
}

/// Best-effort hygiene: staging dirs older than 1 h are dead weight — a
/// pending one-click update expires after 1 h anyway (the WebUI asks the
/// user to re-run Check + Install). Never fails the update.
fn sweep_stale_staging() {
    let cutoff = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_sub(3600);
    if let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) {
        for e in entries.flatten() {
            if !e
                .file_name()
                .to_string_lossy()
                .starts_with("hyprfetch-update-")
            {
                continue;
            }
            let old = e
                .metadata()
                .and_then(|md| md.modified())
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if old < cutoff {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

/// Privilege ladder for a system-owned binary. Each rung is tried in order;
/// the first success wins. Returns [`UpdateError::PasswordRequired`] only
/// after every passwordless rung was declined/unavailable (NonInteractive),
/// or a plain failure for other errors.
fn escalate_and_swap(exe: &Path, staged: &Path, escalation: Escalation) -> Result<(), UpdateError> {
    let mut last_err: Option<UpdateError> = None;
    let rungs: Vec<(&'static str, PrivCmd)> = match escalation {
        // CLI with a TTY: interactive sudo prompts for the password — the
        // historic behaviour (and still the best terminal experience).
        Escalation::Auto => {
            return match find_priv_tool() {
                Some(program) => swap_binary_escalated(
                    exe,
                    &std::fs::read(staged).map_err(UpdateError::Io)?,
                    &PrivCmd {
                        program: program.to_string(),
                        pre_args: Vec::new(),
                    },
                ),
                None => Err(root_needed(exe, true)),
            };
        }
        // Daemon / WebUI: silent rungs first, GUI prompt second.
        Escalation::NonInteractive => {
            let mut v = Vec::new();
            if priv_helper_ready() {
                v.push((
                    "helper",
                    PrivCmd {
                        program: "sudo".to_string(),
                        pre_args: vec!["-n", PRIV_HELPER_PATH],
                    },
                ));
            }
            if which_on_path("sudo") && sudo_n_ok() {
                v.push((
                    "sudo -n",
                    PrivCmd {
                        program: "sudo".to_string(),
                        pre_args: vec!["-n"],
                    },
                ));
            }
            if which_on_path("pkexec") {
                v.push((
                    "pkexec",
                    PrivCmd {
                        program: "pkexec".to_string(),
                        pre_args: Vec::new(),
                    },
                ));
            }
            v
        }
        Escalation::Refuse => return Err(root_needed(exe, false)),
    };

    for (name, tool) in &rungs {
        let res = match *name {
            // The helper takes the staged path DIRECTLY (no `sh -c`): its
            // whitelist only ever touches a `hyprfetch` binary.
            "helper" => swap_via_helper(staged, exe, tool),
            _ => swap_binary_escalated(exe, &std::fs::read(staged).map_err(UpdateError::Io)?, tool),
        };
        match res {
            Ok(()) => return Ok(()),
            Err(e) => {
                tracing::debug!(rung = %name, error = %e, "privileged swap rung failed");
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| UpdateError::PasswordRequired {
        staged: staged.to_string_lossy().into_owned(),
        target: exe.to_string_lossy().into_owned(),
        hint: "no privilege route available".into(),
    }))
}

/// Swap through the one-click helper (`sudo -n /usr/lib/hyprfetch/
/// privileged-update install <staged> <target>`) — no `sh -c`, no prompts.
fn swap_via_helper(staged: &Path, exe: &Path, tool: &PrivCmd) -> Result<(), UpdateError> {
    let out = std::process::Command::new(&tool.program)
        .args(&tool.pre_args)
        .arg("install")
        .arg(staged)
        .arg(exe)
        .output()
        .map_err(|e| UpdateError::Other(format!("cannot run helper: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(UpdateError::Other(format!(
        "privileged helper failed (exit {}): {}",
        out.status.code().unwrap_or(-1),
        stderr.trim()
    )))
}

/// Is the one-click privileged helper installed AND authorized (the
/// sudoers drop-in lets this user run it without a password)?
pub fn priv_helper_ready() -> bool {
    #[cfg(unix)]
    {
        if !Path::new(PRIV_HELPER_PATH).exists() {
            return false;
        }
        std::process::Command::new("sudo")
            .args(["-n", PRIV_HELPER_PATH, "check"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// The root-owned privileged-update helper installed by the one-time
/// setup. Deliberately NARROW: it can only install/remove a file named
/// `hyprfetch` inside `/usr/bin` or `/usr/local/bin` — nothing else.
pub const HELPER_SH: &str = r##"#!/bin/sh
# HyprFetch privileged update helper (installed once by the one-click
# update setup; after that in-app updates never ask for a password).
set -eu
[ "$#" -ge 1 ] || { echo "usage: privileged-update check|install|remove" >&2; exit 64; }
allowed() { case "$1" in /usr/bin/hyprfetch|/usr/local/bin/hyprfetch) return 0 ;; *) return 1 ;; esac; }
case "$1" in
  check)
    exit 0
    ;;
  install)
    [ "$#" -eq 3 ] || { echo "usage: privileged-update install <staged> <target>" >&2; exit 64; }
    allowed "$3" || { echo "refusing target: $3" >&2; exit 65; }
    [ -f "$2" ] || { echo "no staged binary at $2" >&2; exit 66; }
    if [ -f "$3" ]; then mv -f "$3" "$3.old"; fi
    if install -m 0755 "$2" "$3"; then
      rm -f "$2"
    else
      if [ -f "$3.old" ]; then mv -f "$3.old" "$3"; fi
      exit 1
    fi
    ;;
  remove)
    [ "$#" -eq 2 ] || { echo "usage: privileged-update remove <target>" >&2; exit 64; }
    allowed "$2" || { echo "refusing target: $2" >&2; exit 65; }
    rm -f "$2" "$2.old" "$2.new"
    ;;
  *)
    echo "unknown command: $1" >&2
    exit 64
    ;;
esac
"##;

/// The invoking username for the sudoers drop-in (`SUDO_USER`/`USER` env,
/// else `id -un`). Used by the API's one-click-update authorize flow.
#[allow(dead_code)] // wired into /api/update/authorize (hyprfetch-api)
pub fn invoking_user() -> Option<String> {
    if let Ok(u) = std::env::var("SUDO_USER") {
        if !u.is_empty() {
            return Some(u);
        }
    }
    if let Ok(u) = std::env::var("USER") {
        if !u.is_empty() {
            return Some(u);
        }
    }
    std::process::Command::new("id")
        .arg("-un")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Build the ONE-TIME setup script that the terminal window runs under
/// `sudo`: install the helper + the sudoers drop-in (validated with
/// visudo), then finish the PENDING swap with the staged binary. After
/// this runs once, every future in-app update is silent.
pub fn setup_script(user: &str, staged: &Path, target: &Path) -> String {
    let sudoers_line = format!("{user} ALL=(root) NOPASSWD: {PRIV_HELPER_PATH} *");
    format!(
        r##"set -eu
# --- 1. the narrow privileged helper -------------------------------------
mkdir -p /usr/lib/hyprfetch
cat > '{helper}' <<'HYPRFETCH_HELPER_EOF'
{helper_sh}HYPRFETCH_HELPER_EOF
chmod 755 '{helper}'
# --- 2. the sudoers drop-in (validated before it lands) ------------------
LINE='{sudoers_line}'
TMP=$(mktemp)
printf '%s\n' "$LINE" > "$TMP"
if command -v visudo >/dev/null 2>&1; then
  if ! visudo -cf "$TMP" >/dev/null; then
    rm -f "$TMP"
    echo 'sudoers validation failed — aborting (nothing was changed)' >&2
    exit 1
  fi
fi
install -m 0440 -o root -g root "$TMP" '{sudoers}'
rm -f "$TMP"
# --- 3. finish the pending update right now ------------------------------
'{helper}' install '{staged}' '{target}'
echo 'HyprFetch one-click updates enabled — this was the last password.'
"##,
        helper = PRIV_HELPER_PATH,
        helper_sh = HELPER_SH,
        sudoers = SUDOERS_PATH,
        sudoers_line = sudoers_line,
        staged = staged.display(),
        target = target.display(),
    )
}

/// Terminal emulators we can pop the one-time password prompt in, with
/// each terminal's "run command" argument convention. Ordered by how
/// common they are on Hyprland/Wayland desktops first.
const TERMINALS: &[(&str, &[&str])] = &[
    ("kitty", &["-e"]),
    ("alacritty", &["-e"]),
    ("ghostty", &["-e"]),
    ("foot", &[]),
    ("wezterm", &["start", "--always-new-process", "--"]),
    ("konsole", &["-e"]),
    ("gnome-terminal", &["--"]),
    ("xfce4-terminal", &["-x"]),
    ("tilix", &["-e"]),
    ("qterminal", &["-e"]),
    ("lxterminal", &["-e"]),
    ("xterm", &["-e"]),
    ("uxterm", &["-e"]),
    ("st", &["-e"]),
];

/// Pick the first installed terminal and build its command line for
/// running `sh -c <script>`.
#[cfg(unix)]
pub fn terminal_candidate(script: &str) -> Option<(String, Vec<String>)> {
    for (program, pre) in TERMINALS {
        if !which_on_path(program) {
            continue;
        }
        let mut args: Vec<String> = pre.iter().map(|s| s.to_string()).collect();
        args.push("sh".to_string());
        args.push("-c".to_string());
        args.push(script.to_string());
        return Some((program.to_string(), args));
    }
    None
}

/// Spawn a terminal window running `sh -c <script>` (the one-time setup).
/// Fire-and-forget: the daemon polls for the swap completing. Returns the
/// terminal program that was launched.
pub fn spawn_terminal_script(script: &str) -> Result<String, UpdateError> {
    #[cfg(unix)]
    {
        let (program, args) = terminal_candidate(script).ok_or_else(|| {
            UpdateError::Other(
                "no terminal emulator found (kitty/alacritty/foot/ghostty/… not installed) \
                 — run the manual command instead"
                    .into(),
            )
        })?;
        std::process::Command::new(&program)
            .args(&args)
            .spawn()
            .map_err(|e| UpdateError::Other(format!("cannot launch {program}: {e}")))?;
        Ok(program)
    }
    #[cfg(not(unix))]
    {
        let _ = script;
        Err(UpdateError::Other("unsupported platform".into()))
    }
}

/// Build the [`UpdateError::RootNeeded`] error with an actionable hint.
fn root_needed(exe: &Path, no_tool: bool) -> UpdateError {
    let path = exe.to_string_lossy().into_owned();
    let hint = if no_tool {
        "no usable privilege tool: open a terminal and run `sudo hyprfetch update` \
         once, or reinstall with the one-line installer \
         (curl -fsSL https://istias.tech/hyprfetch/updates/install.sh | sh)"
            .to_string()
    } else {
        "open a terminal and run `sudo hyprfetch update` once (the CLI asks for \
         your password and swaps the binary safely), or reinstall with the \
         one-line installer: curl -fsSL https://istias.tech/hyprfetch/updates/install.sh | sh"
            .to_string()
    };
    UpdateError::RootNeeded { path, hint }
}

/// Can the current user replace `exe` in place (direct atomic swap)?
///
/// Probed by actually creating a scratch file next to the binary — that is
/// exactly what the swap needs, so the check cannot lie. Root passes
/// wherever the filesystem is writable; a user fails on pacman/deb/rpm
/// system directories (`/usr/bin`, `/usr/local/bin`, …).
pub fn can_swap_in_place(exe: &Path) -> bool {
    #[cfg(unix)]
    {
        let dir = exe.parent().unwrap_or_else(|| Path::new("."));
        let probe = dir.join(format!(".hyprfetch-update-probe-{}", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
        {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
                true
            }
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = exe;
        true
    }
}

/// Find a privilege-escalation tool on `PATH`: `sudo` first, `doas` fallback.
/// Returns the bare program name so `Command::new(name)` keeps resolving it
/// at spawn time (and tests can shim it via `PATH`).
pub fn find_priv_tool() -> Option<&'static str> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path) {
            for name in ["sudo", "doas"] {
                let candidate = dir.join(name);
                if let Ok(md) = std::fs::metadata(&candidate) {
                    if md.is_file() && md.permissions().mode() & 0o111 != 0 {
                        return Some(name);
                    }
                }
            }
        }
        None
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Which installed package (if any) owns `exe`, per pacman.
/// Returns e.g. `"hyprfetch-bin 0.4.2"`; `None` when pacman is absent,
/// the binary is not packaged, or pacman errors.
pub fn package_owner(exe: &Path) -> Option<String> {
    let out = std::process::Command::new("pacman")
        .arg("-Qo")
        .arg(exe)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    s.split("is owned by")
        .nth(1)
        .map(|rest| rest.trim().trim_end_matches('.').to_string())
}

// ---------------------------------------------------------------------------
// Stale / shadowing copies
//
// A second `hyprfetch` on PATH (e.g. an old install.sh copy in
// /usr/local/bin next to the pacman-managed /usr/bin one) silently wins
// PATH resolution: `hyprfetch --version` keeps reporting the stale build,
// desktop autostart keeps launching it, and freshly-updated installs look
// "broken". These helpers FIND and REMOVE such copies.
// ---------------------------------------------------------------------------

/// A `hyprfetch` executable found on PATH that is not the running binary.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ShadowCopy {
    /// Absolute path of the foreign copy.
    pub path: String,
    /// True when PATH resolves this copy BEFORE the running binary — shells
    /// and desktop launchers keep starting the stale build.
    pub shadows: bool,
    /// Version that copy self-reports (`hyprfetch --version`), if it answers.
    pub version: Option<String>,
    /// pacman package owning the file (`hyprfetch-bin 0.4.4`), if any — such
    /// copies must be removed through the package manager, not `rm`.
    pub owned_by: Option<String>,
}

impl ShadowCopy {
    /// One-line human description used by the CLI and the WebUI.
    pub fn describe(&self) -> String {
        match (&self.version, &self.owned_by) {
            (Some(v), Some(o)) => format!("{} (version {v}, package {o})", self.path),
            (Some(v), None) => format!("{} (version {v})", self.path),
            (None, Some(o)) => format!("{} (package {o})", self.path),
            (None, None) => self.path.clone(),
        }
    }
}

/// Scan `PATH` for other `hyprfetch` executables that shadow or duplicate
/// `active` (the running binary). Symlinks resolving to the same file are
/// not reported. Best-effort: unreadable PATH entries are skipped.
pub async fn shadowed_copies(active: &Path) -> Vec<ShadowCopy> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let Some(path_var) = std::env::var_os("PATH") else {
            return Vec::new();
        };
        let active_canon = std::fs::canonicalize(active).unwrap_or_else(|_| active.to_path_buf());
        let active_dir_canon = active_canon
            .parent()
            .and_then(|d| std::fs::canonicalize(d).ok());

        let entries: Vec<std::path::PathBuf> = std::env::split_paths(&path_var).collect();
        // Index of the running binary's dir on PATH; usize::MAX when it is
        // NOT on PATH (started via absolute path) — then every PATH copy
        // wins resolution and counts as shadowing.
        let active_idx = active_dir_canon
            .as_ref()
            .and_then(|ad| {
                entries
                    .iter()
                    .position(|d| std::fs::canonicalize(d).unwrap_or_else(|_| d.clone()) == *ad)
            })
            .unwrap_or(usize::MAX);

        let mut out: Vec<ShadowCopy> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (idx, dir) in entries.iter().enumerate() {
            let candidate = dir.join("hyprfetch");
            let Ok(md) = std::fs::metadata(&candidate) else {
                continue;
            };
            if !md.is_file() || md.permissions().mode() & 0o111 == 0 {
                continue;
            }
            let cand_canon =
                std::fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
            if cand_canon == active_canon || !seen.insert(cand_canon.clone()) {
                continue; // the running binary itself / duplicate PATH entry
            }
            let shadows = idx < active_idx;
            let version = probe_version(&cand_canon).await;
            let owned_by = package_owner(&cand_canon);
            out.push(ShadowCopy {
                path: cand_canon.to_string_lossy().into_owned(),
                shadows,
                version,
                owned_by,
            });
        }
        out
    }
    #[cfg(not(unix))]
    {
        let _ = active;
        Vec::new()
    }
}

/// Ask a discovered copy for its version (`<path> --version`), with a hard
/// 2s timeout so a foreign/hung file can never stall the caller.
async fn probe_version(path: &Path) -> Option<String> {
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::process::Command::new(path).arg("--version").output(),
    )
    .await
    .ok()?
    .ok()?;
    if !out.status.success() {
        return None;
    }
    let first = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .to_string();
    (!first.is_empty()).then_some(first)
}

/// Remove a stale copy found by [`shadowed_copies`].
///
/// Safety rails:
/// - refuses to remove the running binary (or any path resolving to it),
/// - refuses pacman-owned files — they must go through the package manager,
/// - when the containing directory is not writable, removes it through
///   `priv_cmd` (one `rm -f` under `sudo -n` / `pkexec`); without a tool it
///   fails with an actionable hint.
pub async fn remove_stale_copy(path: &Path, priv_cmd: Option<&PrivCmd>) -> Result<(), UpdateError> {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Ok(cur) = std::env::current_exe() {
        let cur = std::fs::canonicalize(&cur).unwrap_or(cur);
        if path == cur {
            return Err(UpdateError::Other(
                "refusing to remove the running hyprfetch binary".into(),
            ));
        }
    }
    if let Some(owner) = package_owner(&path) {
        return Err(UpdateError::Other(format!(
            "{path} is owned by package `{owner}` — remove it with the package \
             manager (e.g. `sudo pacman -Rns {pkg}`), not directly",
            path = path.display(),
            pkg = owner.split_whitespace().next().unwrap_or("hyprfetch-bin"),
        )));
    }
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            let tool = priv_cmd.ok_or_else(|| {
                UpdateError::Other(format!(
                    "cannot remove {path} — root-owned directory; re-run with \
                     sudo or from the WebUI (pkexec)",
                    path = path.display()
                ))
            })?;
            let script = format!("rm -f {}", sh_quote(&path));
            match tool.run_sh(&script) {
                Ok(s) if s.success() => Ok(()),
                Ok(s) => Err(UpdateError::Other(format!(
                    "privileged removal via {} failed (exit {s})",
                    tool.program
                ))),
                Err(e) => Err(UpdateError::Other(format!(
                    "cannot run {}: {e}",
                    tool.program
                ))),
            }
        }
        Err(e) => Err(UpdateError::Other(format!(
            "cannot remove {path}: {e}",
            path = path.display()
        ))),
    }
}

/// The release archive naming scheme is `hyprfetch-<ver>-<target>.tar.gz`.
/// Derive the target triple candidates for the running binary (Linux-first).
fn target_candidates() -> Vec<String> {
    let arch = std::env::consts::ARCH;
    vec![
        format!("{arch}-unknown-linux-gnu"),
        format!("{arch}-unknown-linux-musl"),
    ]
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Extract only the top-level `hyprfetch` executable from a tarball FILE
/// (streamed — the archive never sits fully in RAM). Same rules as
/// [`extract_binary`]: plain regular file, basename exactly `hyprfetch`,
/// traversal/symlinks refused.
pub fn extract_binary_file(tarball: &Path) -> Result<Vec<u8>, UpdateError> {
    let f = std::fs::File::open(tarball)?;
    let gz = flate2::read::GzDecoder::new(f);
    let mut archive = tar::Archive::new(gz);
    archive.set_preserve_permissions(false);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        if path.file_name().and_then(|f| f.to_str()) != Some("hyprfetch") {
            continue;
        }
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let mut out = Vec::new();
        std::io::copy(&mut entry, &mut out)?;
        if out.is_empty() {
            continue;
        }
        return Ok(out);
    }
    Err(UpdateError::BinaryMissing)
}

/// Extract only the top-level `hyprfetch` executable from the tarball bytes.
/// Refuses path traversal and symlink tricks; returns the file bytes.
pub fn extract_binary(tarball: &[u8]) -> Result<Vec<u8>, UpdateError> {
    let gz = flate2::read::GzDecoder::new(tarball);
    let mut archive = tar::Archive::new(gz);
    archive.set_preserve_permissions(false);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        // Only a plain top-level (or single-dir prefix) regular file whose
        // basename is exactly the binary name is accepted.
        if path.file_name().and_then(|f| f.to_str()) != Some("hyprfetch") {
            continue;
        }
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let mut out = Vec::new();
        std::io::copy(&mut entry, &mut out)?;
        if out.is_empty() {
            continue;
        }
        return Ok(out);
    }
    Err(UpdateError::BinaryMissing)
}

/// Outcome of a successful binary swap.
#[derive(Debug, Serialize)]
pub struct ApplyResult {
    pub current: String,
    pub installed: String,
    pub backup_path: Option<String>,
    pub sha256: String,
    /// True when the swap needed sudo/doas (package-managed system install).
    pub escalated: bool,
    /// True when the binary MOVED to `~/.local/bin` because the old
    /// location was not user-writable (v0.4.9). After this one migration
    /// every later update is a plain in-place swap — passwordless forever.
    pub migrated: bool,
    /// New binary location after a migration (`~/.local/bin/hyprfetch`).
    pub new_path: Option<String>,
    /// PATH fixes applied during migration (rc files, fish universal var).
    pub path_fixes: Vec<String>,
    /// When a stale system copy could NOT be relinked automatically: the
    /// exact one-liner that finishes the job (run once, then silence).
    pub system_fix_hint: Option<String>,
}

/// Replace a system-owned binary via a privilege tool (`sudo`/`doas`).
///
/// Layout: stage the new binary in a user-writable temp dir, then run ONE
/// privileged script (`sudo`/`doas` → `sh -c`) that moves the old binary to
/// `<exe>.old` and installs the staged one as `<exe>` (root-owned, mode
/// 0755). A single script keeps the swap self-recovering: if `install`
/// fails, the script moves the old binary back before exiting, so the
/// system never ends up without a working `hyprfetch`.
pub fn swap_binary_escalated(
    exe: &Path,
    new_bytes: &[u8],
    tool: &PrivCmd,
) -> Result<(), UpdateError> {
    // Stage the new binary where the CURRENT user can write it; the
    // privileged step then installs it into place (root-owned, 0755).
    let stage_dir: PathBuf = std::env::temp_dir().join(format!(
        "hyprfetch-update-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    ));
    std::fs::create_dir_all(&stage_dir)?;
    let staged = stage_dir.join("hyprfetch");
    let staged_result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&staged)?;
        f.write_all(new_bytes)?;
        f.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    })();
    if let Err(e) = staged_result {
        let _ = std::fs::remove_dir_all(&stage_dir);
        return Err(e.into());
    }

    let old_path = exe.with_extension("old");
    let script = escalated_swap_script(exe, &old_path, &staged);

    let run = tool.run_sh(&script);
    // Best-effort staging cleanup (the script removes the staged file on
    // success; the dir may still be left over on failure).
    let _ = std::fs::remove_dir_all(&stage_dir);

    match run {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(UpdateError::Other(format!(
            "privileged swap via {} failed (exit {status}) — {path} was left untouched; \
             run `sudo hyprfetch update` manually if the problem persists",
            tool.program,
            path = exe.display(),
        ))),
        Err(e) => Err(UpdateError::Other(format!(
            "cannot run {}: {e} — install sudo/pkexec or run `sudo hyprfetch update` manually",
            tool.program
        ))),
    }
}

/// POSIX single-quote a path for the privileged `sh -c` script
/// (`'` → `'\''`), so odd install paths can never break out of the quoting.
fn sh_quote(p: &Path) -> String {
    let s = p.to_string_lossy();
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The one privileged script the escalated swap runs:
/// move the old binary aside → install the staged one (root-owned, 0755) →
/// drop the staged copy; if `install` fails, move the old binary BACK so the
/// system is never left without a working `hyprfetch`.
fn escalated_swap_script(exe: &Path, old: &Path, staged: &Path) -> String {
    format!(
        "set -e\nmv {exe} {old}\nif install -m 0755 {new} {exe}; then\n  rm -f {new}\nelse\n  mv {old} {exe}\n  exit 1\nfi\n",
        exe = sh_quote(exe),
        old = sh_quote(old),
        new = sh_quote(staged),
    )
}

/// Replace the binary at `exe` with the STAGED file (same atomic
/// convention as [`swap_binary`]): copy staged → `<exe>.new` (same
/// filesystem as `exe`, so the final rename is atomic), keep the old
/// binary as `<exe>.old` rollback, rename `.new` over `<exe>`.
pub fn swap_binary_from_staged(exe: &Path, staged: &Path) -> Result<(), UpdateError> {
    let new_path = exe.with_extension("new");
    std::fs::copy(staged, &new_path)?;
    {
        let f = std::fs::File::open(&new_path)?;
        f.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new_path, std::fs::Permissions::from_mode(0o755))?;
    }
    // Keep the old binary as rollback.
    let _ = std::fs::remove_file(exe.with_extension("old"));
    if exe.exists() {
        std::fs::rename(exe, exe.with_extension("old"))?;
    }
    std::fs::rename(&new_path, exe)?;
    Ok(())
}

/// Download → verify → swap the running binary atomically.
///
/// Layout: write the new binary as `<exe>.new`, rename the old one to
/// `<exe>.old` (kept as rollback), then rename `.new` over `<exe>`.
/// `rename(2)` on the same filesystem is atomic — a crash mid-swap leaves
/// either the old or the new file, never a truncated one.
pub fn swap_binary(exe: &Path, new_bytes: &[u8]) -> Result<(), UpdateError> {
    let new_path = exe.with_extension("new");
    let old_path = exe.with_extension("old");

    // Write + fsync the new file first.
    {
        let mut f = std::fs::File::create(&new_path)?;
        f.write_all(new_bytes)?;
        f.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new_path, std::fs::Permissions::from_mode(0o755))?;
    }
    // Keep the old binary as rollback.
    let _ = std::fs::remove_file(&old_path);
    if exe.exists() {
        std::fs::rename(exe, &old_path)?;
    }
    std::fs::rename(&new_path, exe)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// User-directory migration (v0.4.9) — the FINAL answer to system-location
// updates. Instead of fighting polkit/pkexec/sudoers for the right to write
// a root-owned directory, the updater MOVES itself to `~/.local/bin`
// (always user-writable, on PATH) and — best effort — turns any old system
// copy into a symlink pointing at it. After one migration every future
// update is a plain in-place swap: no password, no helper, no pkexec, ever.
// ---------------------------------------------------------------------------

/// Outcome of the "binary lives in a non-writable location" branch.
enum SwapOutcome {
    /// The system binary was replaced through a privilege route.
    Escalated,
    /// The binary moved to `~/.local/bin` (+ system-copy dedupe result).
    Migrated(Migration, SystemCopyFix),
}

/// Decide how to replace `exe` with the staged binary when the current
/// location is NOT user-writable:
///
/// 1. passwordless in-place routes (one-click helper, `sudo -n`) — setups
///    that already authorized once keep their exact silent behaviour;
/// 2. package-managed binaries (pacman/deb/rpm own the file) keep the
///    classic privilege ladder — migrating would fight the package manager;
/// 3. everything else MIGRATES to `~/.local/bin` — passwordless, and the
///    last update that ever needs any thought about locations.
fn swap_or_migrate(
    exe: &Path,
    staged: &Path,
    escalation: Escalation,
) -> Result<SwapOutcome, UpdateError> {
    if escalation == Escalation::Refuse {
        return Err(root_needed(exe, false));
    }

    // 1. Passwordless in-place routes (never ask, never block).
    if priv_helper_ready() {
        let cmd = PrivCmd {
            program: "sudo".to_string(),
            pre_args: vec!["-n", PRIV_HELPER_PATH],
        };
        if swap_via_helper(staged, exe, &cmd).is_ok() {
            return Ok(SwapOutcome::Escalated);
        }
    }
    if which_on_path("sudo") && sudo_n_ok() {
        let bytes = std::fs::read(staged).map_err(UpdateError::Io)?;
        let cmd = PrivCmd {
            program: "sudo".to_string(),
            pre_args: vec!["-n"],
        };
        if swap_binary_escalated(exe, &bytes, &cmd).is_ok() {
            return Ok(SwapOutcome::Escalated);
        }
    }

    // 2. Package-managed installs: the file belongs to pacman/deb/rpm.
    if package_owner(exe).is_some() {
        return escalate_and_swap(exe, staged, escalation).map(|_| SwapOutcome::Escalated);
    }

    // 3. THE FINAL ROUTE: move to ~/.local/bin (no password, ever), then
    //    turn any stale system copy into a link to the new location.
    match migrate_to_user_bin(staged) {
        Ok(m) => {
            let fix = link_system_copies_to_user(
                &system_copy_paths_from_env(),
                &m.new_path,
                escalation == Escalation::Auto, // only the CLI may prompt
            );
            Ok(SwapOutcome::Migrated(m, fix))
        }
        Err(migrate_err) => {
            // No $HOME / unusable user dir → last resort: the classic ladder.
            match escalate_and_swap(exe, staged, escalation) {
                Ok(()) => Ok(SwapOutcome::Escalated),
                Err(e) => Err(if matches!(e, UpdateError::PasswordRequired { .. }) {
                    e
                } else {
                    migrate_err
                }),
            }
        }
    }
}

/// The canonical per-user install directory (`$HOME/.local/bin`).
pub fn user_bin_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())?;
    Some(home.join(".local").join("bin"))
}

/// What one migration changed on disk.
#[derive(Debug, Clone, Serialize)]
pub struct Migration {
    /// Where the new binary now lives (`~/.local/bin/hyprfetch`).
    pub new_path: PathBuf,
    /// Rollback copy of the previous binary, when one existed.
    pub backup_path: Option<PathBuf>,
    /// PATH fixes applied (rc files / fish universal var), human-readable.
    pub path_fixes: Vec<String>,
}

/// Move the staged binary into `~/.local/bin` — atomic (write `.new` →
/// rename), keeps the previous binary as `.old` rollback, then makes sure
/// the directory is on PATH for the user's future shells.
pub fn migrate_to_user_bin(staged: &Path) -> Result<Migration, UpdateError> {
    let bin_dir = user_bin_dir().ok_or_else(|| {
        UpdateError::Other("cannot determine $HOME — cannot migrate to ~/.local/bin".into())
    })?;
    migrate_to_user_bin_into(&bin_dir, staged)
}

/// [`migrate_to_user_bin`] with an explicit target directory (testable).
pub fn migrate_to_user_bin_into(bin_dir: &Path, staged: &Path) -> Result<Migration, UpdateError> {
    std::fs::create_dir_all(bin_dir)?;
    let target = bin_dir.join("hyprfetch");
    let new_path = target.with_extension("new");
    std::fs::copy(staged, &new_path)?;
    {
        let f = std::fs::File::open(&new_path)?;
        f.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new_path, std::fs::Permissions::from_mode(0o755))?;
    }
    let mut backup = None;
    if target.exists() {
        let old = target.with_extension("old");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&target, &old)?;
        backup = Some(old);
    }
    std::fs::rename(&new_path, &target)?;
    let path_fixes = ensure_user_path(bin_dir);
    Ok(Migration {
        new_path: target,
        backup_path: backup,
        path_fixes,
    })
}

/// Make sure `~/.local/bin` is on PATH for the user's future shells:
/// a guarded block in `~/.profile` / `~/.bashrc` / `~/.zshrc` (POSIX
/// shells) plus `fish_add_path -U` (universal variable — persists across
/// fish sessions) and a guarded block in `~/.config/fish/config.fish`.
/// Returns the files/vars that were CHANGED (empty when everything was
/// already in place). Never fails the update.
pub fn ensure_user_path(bin_dir: &Path) -> Vec<String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_default();
    ensure_user_path_under(&home, bin_dir)
}

/// [`ensure_user_path`] with an explicit home — the testable core.
pub fn ensure_user_path_under(home: &Path, bin_dir: &Path) -> Vec<String> {
    let mut touched = Vec::new();
    let bin_str = bin_dir.to_string_lossy().into_owned();

    // POSIX shells: one guarded export line per rc file.
    for name in [".profile", ".bashrc", ".zshrc"] {
        let f = home.join(name);
        match std::fs::read_to_string(&f) {
            Ok(content) if content.contains(".local/bin") => {} // already covered
            Ok(content) => {
                let mut next = content;
                if !next.ends_with('\n') {
                    next.push('\n');
                }
                next.push_str(&posix_path_block(&bin_str));
                if std::fs::write(&f, next).is_ok() {
                    touched.push(format!("~/{}", name));
                }
            }
            Err(_) => {
                // Missing file: create it (a fresh rc entry is harmless and
                // helps shells that DO read it).
                if std::fs::write(&f, posix_path_block(&bin_str)).is_ok() {
                    touched.push(format!("~/{} (new)", name));
                }
            }
        }
    }

    // fish: universal variable right now (persists across sessions), plus a
    // guarded block in config.fish for shells that never see our `fish -c`
    // (and for fresh installs where fish is not yet on PATH here).
    if which_on_path("fish") {
        let ok = std::process::Command::new("fish")
            .args(["-c", &format!("fish_add_path -U {}", sh_quote(bin_dir))])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            touched.push("fish (universal PATH)".to_string());
        }
    }
    let fish_cfg = home.join(".config").join("fish").join("config.fish");
    match std::fs::read_to_string(&fish_cfg) {
        Ok(content) if content.contains(".local/bin") => {}
        Ok(content) => {
            let mut next = content;
            if !next.ends_with('\n') {
                next.push('\n');
            }
            next.push_str(&fish_path_block(&bin_str));
            if std::fs::write(&fish_cfg, next).is_ok() {
                touched.push("~/.config/fish/config.fish".to_string());
            }
        }
        Err(_) => {
            // Only create config.fish when fish is actually in use.
            if which_on_path("fish")
                && std::fs::create_dir_all(fish_cfg.parent().unwrap_or(home)).is_ok()
                && std::fs::write(&fish_cfg, fish_path_block(&bin_str)).is_ok()
            {
                touched.push("~/.config/fish/config.fish (new)".to_string());
            }
        }
    }

    touched
}

/// Guarded POSIX block appended to rc files (idempotent via the markers).
fn posix_path_block(bin: &str) -> String {
    format!(
        "# >>> hyprfetch PATH >>> (added by the HyprFetch installer)\n\
         export PATH=\"{bin}:$PATH\"\n\
         # <<< hyprfetch PATH <<<\n"
    )
}

/// Guarded fish block appended to config.fish.
fn fish_path_block(bin: &str) -> String {
    format!(
        "# >>> hyprfetch PATH >>> (added by the HyprFetch installer)\n\
         fish_add_path \"{bin}\"\n\
         # <<< hyprfetch PATH <<<\n"
    )
}

/// Result of turning old system copies into links to the user install.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SystemCopyFix {
    /// No system copy existed — nothing to do.
    NotNeeded,
    /// Every system path already resolves to the user binary.
    AlreadyLinked,
    /// At least one system path now links to the user binary.
    Linked { via: String, linked: Vec<String> },
    /// A package manager owns a copy — untouched (remove it via the
    /// package manager; `pacman -Rns hyprfetch-bin` and friends).
    PackageOwned { owner: String, path: String },
    /// A copy remains and could not be replaced — `hint` carries the exact
    /// one-liner that finishes the job.
    Failed { path: String, hint: String },
}

impl SystemCopyFix {
    /// The one-liner to run when a system copy could not be linked.
    pub fn hint(&self) -> Option<String> {
        match self {
            SystemCopyFix::Failed { hint, .. } => Some(hint.clone()),
            _ => None,
        }
    }
}

/// System locations older install.sh builds may have used. Test override:
/// `HYPRFETCH_SYSTEM_COPY` (colon-separated) replaces them.
pub fn system_copy_paths_from_env() -> Vec<PathBuf> {
    std::env::var_os("HYPRFETCH_SYSTEM_COPY")
        .map(|v| {
            v.to_string_lossy()
                .split(':')
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_else(default_system_copy_paths)
}

pub fn default_system_copy_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/local/bin/hyprfetch"),
        PathBuf::from("/usr/bin/hyprfetch"),
    ]
}

/// Replace every stale system copy with a SYMLINK to the user binary, so
/// each old PATH entry keeps working AND nothing can shadow the fresh
/// install. Passwordless routes first (`sudo -n`, then `pkexec`); plain
/// filesystem ops when the directory is actually writable; interactive
/// `sudo` only when the caller owns a TTY (the CLI — a daemon must never
/// block on a prompt). A package-owned copy is never touched.
///
/// Uses [`system_copy_paths_from_env`] (honours the test override).
pub fn link_system_copy_to_user(user_bin: &Path) -> SystemCopyFix {
    link_system_copies_to_user(
        &system_copy_paths_from_env(),
        user_bin,
        std::io::IsTerminal::is_terminal(&std::io::stdin()),
    )
}

/// [`link_system_copy_to_user`] with explicit paths — the testable core.
pub fn link_system_copies_to_user(
    system_paths: &[PathBuf],
    user_bin: &Path,
    allow_prompt: bool,
) -> SystemCopyFix {
    let user_target = std::fs::canonicalize(user_bin).unwrap_or_else(|_| user_bin.to_path_buf());
    let mut todo: Vec<PathBuf> = Vec::new();
    let mut any_present = false;
    for p in system_paths {
        let Ok(md) = std::fs::symlink_metadata(p) else {
            continue;
        };
        let _ = md;
        any_present = true;
        // Already pointing at the user binary (symlink or hardlink)?
        if std::fs::canonicalize(p)
            .map(|c| c == user_target)
            .unwrap_or(false)
        {
            continue;
        }
        if let Some(owner) = package_owner(p) {
            return SystemCopyFix::PackageOwned {
                owner,
                path: p.to_string_lossy().into_owned(),
            };
        }
        todo.push(p.clone());
    }
    if todo.is_empty() {
        return if any_present {
            SystemCopyFix::AlreadyLinked
        } else {
            SystemCopyFix::NotNeeded
        };
    }
    let mut linked = Vec::new();
    let mut via = String::new();
    for p in &todo {
        match link_one_system_copy(p, user_bin, allow_prompt) {
            Ok(how) => {
                if via.is_empty() {
                    via = how.clone();
                }
                linked.push(format!(
                    "{} → {} ({})",
                    p.display(),
                    user_bin.display(),
                    how
                ));
            }
            Err(hint) => {
                return SystemCopyFix::Failed {
                    path: p.to_string_lossy().into_owned(),
                    hint,
                }
            }
        }
    }
    SystemCopyFix::Linked { via, linked }
}

/// Link ONE system path to the user binary. Tries: direct filesystem ops →
/// `sudo -n` → `pkexec` → interactive `sudo` (only with a TTY). Returns
/// how it linked, or the exact one-liner for the user.
fn link_one_system_copy(
    system: &Path,
    user_bin: &Path,
    allow_prompt: bool,
) -> Result<String, String> {
    let script = link_script(system, user_bin);

    // 1. The directory is actually writable by us — plain ops suffice.
    if can_swap_in_place(system) && direct_link(system, user_bin).is_ok() {
        return Ok("direct".to_string());
    }
    // 2. Passwordless sudo.
    if which_on_path("sudo") {
        let cmd = PrivCmd {
            program: "sudo".to_string(),
            pre_args: vec!["-n"],
        };
        if cmd.run_sh(&script).map(|s| s.success()).unwrap_or(false) {
            return Ok("sudo".to_string());
        }
    }
    // 3. pkexec (may pop a GUI password prompt via the polkit agent).
    if which_on_path("pkexec") {
        let cmd = PrivCmd {
            program: "pkexec".to_string(),
            pre_args: Vec::new(),
        };
        if cmd.run_sh(&script).map(|s| s.success()).unwrap_or(false) {
            return Ok("pkexec".to_string());
        }
    }
    // 4. Interactive sudo — the CLI owns a TTY and may ask once.
    if allow_prompt && which_on_path("sudo") {
        let cmd = PrivCmd {
            program: "sudo".to_string(),
            pre_args: Vec::new(),
        };
        if cmd.run_sh(&script).map(|s| s.success()).unwrap_or(false) {
            return Ok("sudo (password)".to_string());
        }
    }
    Err(link_one_liner(system, user_bin))
}

/// Direct (unprivileged) relink of one system path.
fn direct_link(system: &Path, user_bin: &Path) -> std::io::Result<()> {
    let _ = std::fs::remove_file(system);
    let _ = std::fs::remove_file(system.with_extension("old"));
    let _ = std::fs::remove_file(system.with_extension("new"));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(user_bin, system)?;
    }
    #[cfg(not(unix))]
    {
        let _ = (system, user_bin);
        unreachable!()
    }
    Ok(())
}

/// The one privileged script for the relink: drop the old real file (and
/// any .old/.new leftovers), then symlink to the user binary.
fn link_script(system: &Path, user_bin: &Path) -> String {
    format!(
        "set -e\nrm -f {sys} {old} {new}\nln -sf {user} {sys}\n",
        sys = sh_quote(system),
        old = sh_quote(&system.with_extension("old")),
        new = sh_quote(&system.with_extension("new")),
        user = sh_quote(user_bin),
    )
}

/// The copy-paste one-liner when every automatic route failed.
fn link_one_liner(system: &Path, user_bin: &Path) -> String {
    format!(
        "sudo rm -f {sys} {old} {new} && sudo ln -sf {user} {sys}",
        sys = sh_quote(system),
        old = sh_quote(&system.with_extension("old")),
        new = sh_quote(&system.with_extension("new")),
        user = sh_quote(user_bin),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(version_newer("0.3.1", "0.3.0"));
        assert!(version_newer("v0.4.0", "0.3.99"));
        assert!(version_newer("1.0", "0.9.9"));
        assert!(!version_newer("0.3.1", "0.3.1"));
        assert!(!version_newer("0.2.0", "0.3.0"));
        assert!(!version_newer("0.3", "0.3.0"));
    }

    #[test]
    fn extract_binary_safe() {
        // Build a tar.gz in-memory with two entries; only `hyprfetch` wins.
        let mut builder = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_size(4);
        h.set_cksum();
        builder
            .append_data(&mut h, "hyprfetch", &b"BIN!"[..])
            .unwrap();
        let mut h2 = tar::Header::new_gnu();
        h2.set_size(6);
        h2.set_cksum();
        builder
            .append_data(&mut h2, "README.md", &b"decoy!"[..])
            .unwrap();
        let tar_bytes = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar_bytes).unwrap();
        let tarball = gz.finish().unwrap();

        let bin = extract_binary(&tarball).unwrap();
        assert_eq!(bin, b"BIN!");
        // Decoy must not win even though it appears later.
        assert_ne!(bin, b"decoy!");
        // Non-binary archives report BinaryMissing.
        let mut b2 = tar::Builder::new(Vec::new());
        let mut h3 = tar::Header::new_gnu();
        h3.set_size(6);
        h3.set_cksum();
        b2.append_data(&mut h3, "README.md", &b"decoy!"[..])
            .unwrap();
        let tar_bytes2 = b2.into_inner().unwrap();
        let mut gz2 = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz2.write_all(&tar_bytes2).unwrap();
        assert!(matches!(
            extract_binary(&gz2.finish().unwrap()),
            Err(UpdateError::BinaryMissing)
        ));
    }

    #[test]
    fn swap_binary_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("hyprfetch");
        std::fs::write(&exe, b"old-binary").unwrap();

        swap_binary(&exe, b"new-binary").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new-binary");
        assert_eq!(
            std::fs::read(exe.with_extension("old")).unwrap(),
            b"old-binary",
            "previous binary kept as rollback"
        );
        assert!(!exe.with_extension("new").exists(), "temp file consumed");
    }

    #[cfg(unix)]
    #[test]
    fn can_swap_in_place_probes_the_real_directory() {
        use std::os::unix::fs::PermissionsExt;
        // Writable dir → probe succeeds.
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("hyprfetch");
        std::fs::write(&exe, b"x").unwrap();
        assert!(can_swap_in_place(&exe), "writable dir must allow the swap");

        // Read-only dir → probe fails (same EPERM a pacman-owned /usr/bin
        // gives a user process).
        let ro = tempfile::tempdir().unwrap();
        let exe_ro = ro.path().join("hyprfetch");
        std::fs::write(&exe_ro, b"x").unwrap();
        std::fs::set_permissions(ro.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        assert!(
            !can_swap_in_place(&exe_ro),
            "read-only dir must require escalation"
        );
        std::fs::set_permissions(ro.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn escalated_swap_runs_one_self_recovering_script() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let bindir = tmp.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        let exe = bindir.join("hyprfetch");
        std::fs::write(&exe, b"old-binary").unwrap();

        // A stand-in "sudo" that just executes the privileged script as the
        // current user — exercises the exact same Command plumbing and the
        // exact same sh script the real sudo runs (only the euid differs).
        let shimdir = tempfile::tempdir().unwrap();
        let shim = shimdir.path().join("fake-sudo");
        std::fs::write(&shim, "#!/bin/sh\nexec \"$@\"\n").unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();

        swap_binary_escalated(
            &exe,
            b"new-binary",
            &PrivCmd {
                program: shim.to_str().unwrap().to_string(),
                pre_args: Vec::new(),
            },
        )
        .unwrap();

        assert_eq!(std::fs::read(&exe).unwrap(), b"new-binary");
        assert_eq!(
            std::fs::read(exe.with_extension("old")).unwrap(),
            b"old-binary",
            "previous binary kept as rollback"
        );
        let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "installed binary must be 0755");
        // No staging leftovers in the temp dir.
        let leftovers: Vec<_> = std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("hyprfetch-update-")
            })
            .collect();
        assert!(leftovers.is_empty(), "staging dir must be cleaned up");
    }

    #[cfg(unix)]
    #[test]
    fn escalated_swap_failure_leaves_binary_untouched() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("hyprfetch");
        std::fs::write(&exe, b"old-binary").unwrap();

        // A "sudo" that always fails WITHOUT running the script (like sudo
        // hitting "a password is required" in a non-tty) — nothing may
        // change on disk.
        let shimdir = tempfile::tempdir().unwrap();
        let shim = shimdir.path().join("fake-sudo");
        std::fs::write(&shim, "#!/bin/sh\nexit 3\n").unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();

        let err = swap_binary_escalated(
            &exe,
            b"new-binary",
            &PrivCmd {
                program: shim.to_str().unwrap().to_string(),
                pre_args: Vec::new(),
            },
        )
        .unwrap_err();
        assert!(
            err.to_string().starts_with("privileged swap via ") && err.to_string().ends_with(
                "was left untouched; run `sudo hyprfetch update` manually if the problem persists"
            ),
            "unexpected error: {err}"
        );
        assert_eq!(
            std::fs::read(&exe).unwrap(),
            b"old-binary",
            "failed escalation must not destroy the installed binary"
        );
        assert!(
            !exe.with_extension("old").exists(),
            "rollback restored the original"
        );
    }

    #[test]
    fn escalated_script_rolls_back_when_install_fails() {
        // Run the REAL privileged script with a staged path that does not
        // exist → `install` fails → the script must move the old binary back
        // before exiting non-zero.
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("hyprfetch");
        std::fs::write(&exe, b"old-binary").unwrap();
        let old = exe.with_extension("old");
        let ghost = tmp.path().join("does-not-exist");

        let script = escalated_swap_script(&exe, &old, &ghost);
        let st = std::process::Command::new("sh")
            .args(["-c", &script])
            .status()
            .unwrap();
        assert!(!st.success(), "script must report failure");
        assert_eq!(
            std::fs::read(&exe).unwrap(),
            b"old-binary",
            "rollback must restore the previous binary"
        );
        assert!(!old.exists(), "rollback removes the .old copy");
    }

    #[test]
    fn escalated_script_shape() {
        let s = escalated_swap_script(
            Path::new("/usr/bin/hyprfetch"),
            Path::new("/usr/bin/hyprfetch.old"),
            Path::new("/tmp/stage/hyprfetch"),
        );
        assert!(s.starts_with("set -e\n"));
        assert!(s.contains("mv '/usr/bin/hyprfetch' '/usr/bin/hyprfetch.old'"));
        assert!(s.contains("install -m 0755 '/tmp/stage/hyprfetch' '/usr/bin/hyprfetch'"));
        assert!(s.contains("mv '/usr/bin/hyprfetch.old' '/usr/bin/hyprfetch'"));
    }

    #[test]
    fn sh_quote_survives_hostile_paths() {
        let weird = Path::new("/opt/my 'weird'/hyprfetch");
        let q = sh_quote(weird);
        assert_eq!(q, "'/opt/my '\\''weird'\\''/hyprfetch'");
        // Round-trip through sh: echo the quoted string back.
        let out = std::process::Command::new("sh")
            .args(["-c", &format!("printf '%s' {q}")])
            .output()
            .unwrap();
        assert_eq!(out.stdout, weird.as_os_str().as_encoded_bytes());
    }

    #[cfg(unix)]
    #[test]
    fn find_priv_tool_finds_sudo() {
        // The CI/sandbox image has sudo on PATH; if it ever disappears this
        // test just downgrades to "no tool found" (both outcomes are valid
        // — what matters is no panic and no false sudo).
        let tool = find_priv_tool();
        if which_sudo_exists() {
            assert_eq!(tool, Some("sudo"));
        }
    }

    #[cfg(unix)]
    fn which_sudo_exists() -> bool {
        std::process::Command::new("sh")
            .args(["-c", "command -v sudo >/dev/null 2>&1"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[test]
    fn package_owner_is_none_without_pacman_or_unowned_file() {
        // tempfile paths are never pacman-owned; pacman itself is usually
        // absent on CI. Either way the function must return None cleanly.
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("hyprfetch");
        std::fs::write(&exe, b"x").unwrap();
        assert!(package_owner(&exe).is_none());
    }

    #[test]
    fn manifest_url_join() {
        assert_eq!(
            manifest_url("https://istias.tech/hyprfetch/updates/"),
            "https://istias.tech/hyprfetch/updates/latest.json"
        );
        assert_eq!(
            manifest_url("https://istias.tech/hyprfetch/updates"),
            "https://istias.tech/hyprfetch/updates/latest.json"
        );
    }

    #[test]
    fn channel_effective_url() {
        let mut cfg = UpdateConfig::default();
        assert_eq!(cfg.effective_channel(), Some(DEFAULT_CHANNEL_URL));
        cfg.channel_url = String::new();
        assert_eq!(cfg.effective_channel(), None, "empty disables the updater");
        cfg.channel_url = "  ".into();
        assert_eq!(cfg.effective_channel(), None, "blank disables the updater");
    }

    fn sample_manifest() -> &'static str {
        r#"{
            "version": "9.9.9",
            "tag": "v9.9.9",
            "published_at": "2026-09-29T12:00:00Z",
            "notes_url": "https://istias.tech/hyprfetch/updates",
            "assets": {
                "x86_64-unknown-linux-gnu": {
                    "url": "https://istias.tech/hyprfetch/updates/9.9.9/hyprfetch-9.9.9-linux-x64.tar.gz",
                    "sha256": "abc123",
                    "size": 42
                },
                "aarch64-unknown-linux-gnu": {
                    "url": "https://istias.tech/hyprfetch/updates/9.9.9/hyprfetch-9.9.9-linux-arm64.tar.gz",
                    "sha256": "def456",
                    "size": 43
                }
            }
        }"#
    }

    #[test]
    fn manifest_parses_and_picks_host_asset() {
        let m: ChannelManifest = serde_json::from_str(sample_manifest()).unwrap();
        assert_eq!(m.version, "9.9.9");
        assert_eq!(m.tag, "v9.9.9");
        let picked = pick_channel_asset(&m.assets);
        assert!(picked.is_some(), "host arch must find its asset");
        let (target, a) = picked.unwrap();
        assert_eq!(a.sha256, "abc123");
        assert!(target.starts_with(std::env::consts::ARCH));
    }

    #[test]
    fn manifest_requires_version_and_assets() {
        // Missing `assets` -> parse error; missing `version` -> parse error.
        assert!(
            serde_json::from_str::<ChannelManifest>(r#"{"tag":"v1.0.0","assets":{}}"#).is_err()
        );
        assert!(serde_json::from_str::<ChannelManifest>(r#"{"version":"1.0.0"}"#).is_err());
    }

    #[test]
    fn update_check_serializes_for_the_web_ui() {
        let chk = UpdateCheck {
            current: "0.4.0".into(),
            latest: "0.5.0".into(),
            available: true,
            published_at: None,
            release_url: None,
            asset: Some(AssetInfo {
                name: "hyprfetch-0.5.0-linux-x64.tar.gz".into(),
                size: 42,
            }),
            channel: Some(DEFAULT_CHANNEL_URL.into()),
        };
        let v = serde_json::to_value(&chk).unwrap();
        assert_eq!(v["available"], true);
        assert_eq!(v["asset"]["name"], "hyprfetch-0.5.0-linux-x64.tar.gz");
        assert!(v.get("via_git").is_none(), "git tier is gone");
        assert!(v.get("via_channel").is_none(), "channel is the only tier");
    }

    // -- stale / shadowing copies ------------------------------------------

    /// Serializes tests that mutate `PATH` (process-global state). Async
    /// mutex: guards are held across `.await` points by design.
    static PATH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// `active` dir at `a/`, stale copy at `s/`, PATH = `s:a:rest`.
    /// The stale copy prints a version so `probe_version` has data.
    #[cfg(unix)]
    fn shadow_fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let a_dir = tmp.path().join("a");
        let s_dir = tmp.path().join("s");
        std::fs::create_dir_all(&a_dir).unwrap();
        std::fs::create_dir_all(&s_dir).unwrap();
        let active = a_dir.join("hyprfetch");
        let stale = s_dir.join("hyprfetch");
        std::fs::write(&active, b"#!/bin/sh\necho \"hyprfetch 9.9.9\"\n").unwrap();
        std::fs::write(&stale, b"#!/bin/sh\necho \"hyprfetch 0.1.0\"\n").unwrap();
        for p in [&active, &stale] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        (tmp, active, stale)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shadowed_copies_finds_and_orders_foreign_copy() {
        let _guard = PATH_LOCK.lock().await;
        let (tmp, active, stale) = shadow_fixture();
        let old_path = std::env::var_os("PATH").unwrap();
        std::env::set_var(
            "PATH",
            format!(
                "{}:{}",
                stale.parent().unwrap().display(),
                active.parent().unwrap().display()
            ),
        );
        let found = shadowed_copies(&active).await;
        std::env::set_var("PATH", old_path);

        assert_eq!(found.len(), 1, "exactly the one foreign copy: {found:?}");
        let c = &found[0];
        assert!(c.shadows, "stale dir precedes the active dir on PATH");
        assert_eq!(c.version.as_deref(), Some("hyprfetch 0.1.0"));
        assert!(c.owned_by.is_none(), "tempfile is never pacman-owned");
        assert!(c.describe().contains(&c.path));
        drop(tmp);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shadowed_copies_reports_shadowed_active_copy() {
        let _guard = PATH_LOCK.lock().await;
        let (tmp, active, stale) = shadow_fixture();
        let old_path = std::env::var_os("PATH").unwrap();
        // Active dir FIRST — the foreign copy no longer shadows, but is
        // still reported (it is a duplicate install that confuses users).
        std::env::set_var(
            "PATH",
            format!(
                "{}:{}",
                active.parent().unwrap().display(),
                stale.parent().unwrap().display()
            ),
        );
        let found = shadowed_copies(&active).await;
        std::env::set_var("PATH", old_path);

        assert_eq!(found.len(), 1);
        assert!(!found[0].shadows, "foreign copy sits AFTER the active dir");
        drop(tmp);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shadowed_copies_ignores_the_active_binary_itself() {
        let _guard = PATH_LOCK.lock().await;
        let (tmp, active, _stale) = shadow_fixture();
        let old_path = std::env::var_os("PATH").unwrap();
        std::env::set_var("PATH", active.parent().unwrap().display().to_string());
        let found = shadowed_copies(&active).await;
        std::env::set_var("PATH", old_path);
        assert!(found.is_empty(), "self must never be reported: {found:?}");
        drop(tmp);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn remove_stale_copy_direct_and_refuses_running_binary() {
        let _guard = PATH_LOCK.lock().await;
        let (tmp, _active, stale) = shadow_fixture();
        remove_stale_copy(&stale, None).await.unwrap();
        assert!(!stale.exists(), "user-owned copy removed directly");

        // The running binary (this test executable) must never be removable.
        let cur = std::env::current_exe().unwrap();
        let err = remove_stale_copy(&cur, None).await.unwrap_err();
        assert!(err.to_string().contains("refusing to remove the running"));
        drop(tmp);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn remove_stale_copy_routes_readonly_dir_to_the_priv_tool() {
        use std::os::unix::fs::PermissionsExt;
        let (tmp, active, _stale) = shadow_fixture();
        let ro_dir = tmp.path().join("ro");
        std::fs::create_dir_all(&ro_dir).unwrap();
        let target = ro_dir.join("hyprfetch");
        std::fs::write(&target, b"stale").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

        // A stand-in "sudo" that executes the privileged script as the
        // current user (it cannot grant root here, but it exercises the
        // exact Command plumbing the real sudo runs).
        let shimdir = tempfile::tempdir().unwrap();
        let shim = shimdir.path().join("fake-sudo");
        std::fs::write(&shim, "#!/bin/sh\nexec \"$@\"\n").unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tool = PrivCmd {
            program: shim.to_str().unwrap().to_string(),
            pre_args: Vec::new(),
        };

        // With a tool: the EPERM branch forwards the removal into the
        // privileged script (its non-root exit surfaces as a tool failure,
        // NOT the "no tool" hint).
        let err = remove_stale_copy(&target, Some(&tool)).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.starts_with("privileged removal via") && msg.contains("fake-sudo"),
            "unexpected error: {msg}"
        );

        // Without a tool: same EPERM branch must fail with the actionable
        // "root-owned directory" hint instead of trying to delete silently.
        let err = remove_stale_copy(&target, None).await.unwrap_err();
        assert!(err.to_string().contains("root-owned directory"));
        std::fs::set_permissions(&ro_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = active;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn noninteractive_discovery_prefers_passwordless_sudo_shim() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = PATH_LOCK.lock().await;
        let old_path = std::env::var_os("PATH").unwrap();
        // Shim sudo: `sudo -n true` → 0 (passwordless probe), anything else
        // through. No pkexec on this PATH. `true` is handled explicitly
        // because this shim-only PATH has no coreutils on it.
        let tmp = tempfile::tempdir().unwrap();
        let sudo = tmp.path().join("sudo");
        std::fs::write(
            &sudo,
            "#!/bin/sh\nif [ \"$1\" = \"-n\" ]; then shift; fi\nif [ \"$1\" = \"true\" ]; then exit 0; fi\nexec \"$@\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&sudo, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("PATH", tmp.path().display().to_string());
        let tool = find_priv_tool_noninteractive();
        std::env::set_var("PATH", old_path);
        assert_eq!(
            tool,
            Some(PrivCmd {
                program: "sudo".to_string(),
                pre_args: vec!["-n"],
            })
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn noninteractive_discovery_falls_back_to_pkexec() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = PATH_LOCK.lock().await;
        let old_path = std::env::var_os("PATH").unwrap();
        // A sudo whose passwordless probe FAILS (like a real desktop sudo
        // without NOPASSWD) + a pkexec present → pkexec must win.
        let tmp = tempfile::tempdir().unwrap();
        let sudo = tmp.path().join("sudo");
        std::fs::write(&sudo, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&sudo, std::fs::Permissions::from_mode(0o755)).unwrap();
        let pkexec = tmp.path().join("pkexec");
        std::fs::write(&pkexec, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&pkexec, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("PATH", tmp.path().display().to_string());
        let tool = find_priv_tool_noninteractive();
        std::env::set_var("PATH", old_path);
        assert_eq!(
            tool,
            Some(PrivCmd {
                program: "pkexec".to_string(),
                pre_args: Vec::new(),
            })
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn noninteractive_discovery_none_without_tools() {
        let _guard = PATH_LOCK.lock().await;
        let old_path = std::env::var_os("PATH").unwrap();
        let tmp = tempfile::tempdir().unwrap(); // empty PATH dir: no sudo/pkexec
        std::env::set_var("PATH", tmp.path().display().to_string());
        let tool = find_priv_tool_noninteractive();
        std::env::set_var("PATH", old_path);
        assert_eq!(tool, None, "empty PATH must yield no tool");
    }

    // -- robust download engine (v0.4.7) -----------------------------------

    use std::sync::Arc;

    /// Minimal raw-socket HTTP server for download tests. Supports Range
    /// requests and three failure modes, all selected per-connection:
    ///   • drop_after: send only N body bytes then hang up (dropped stream)
    ///   • stall: headers only, body never arrives (tests the idle timeout)
    ///   • corrupt: flip body bytes so the sha256 will mismatch
    /// Every request's Range header is recorded for resume assertions.
    #[cfg(unix)]
    struct MockAsset {
        addr: String,
        requests: Arc<std::sync::Mutex<Vec<String>>>,
        stall: Arc<std::sync::atomic::AtomicBool>,
    }

    #[cfg(unix)]
    impl MockAsset {
        fn spawn(body: Vec<u8>) -> Self {
            Self::spawn_with(body, usize::MAX, false, false)
        }

        fn spawn_with(body: Vec<u8>, drop_after: usize, stall: bool, corrupt: bool) -> Self {
            use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap().to_string();
            let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
            let stall_flag = Arc::new(AtomicBool::new(stall));
            let corrupt_flag = Arc::new(AtomicBool::new(corrupt));
            let drop_after = Arc::new(AtomicUsize::new(drop_after));
            let drops_left = Arc::new(AtomicUsize::new(1)); // only the FIRST conn drops
            let req2 = Arc::clone(&requests);
            let req_thread = Arc::clone(&req2);
            let stall_thread = Arc::clone(&stall_flag);
            let corrupt_thread = Arc::clone(&corrupt_flag);
            let drop_thread = Arc::clone(&drop_after);
            let drops_thread = Arc::clone(&drops_left);
            std::thread::spawn(move || {
                let req2 = req_thread;
                let stall_flag = stall_thread;
                let corrupt_flag = corrupt_thread;
                let drop_after = drop_thread;
                let drops_left = drops_thread;
                for conn in listener.incoming() {
                    let mut conn = match conn {
                        Ok(c) => c,
                        Err(_) => break,
                    };
                    let reqs = Arc::clone(&req2);
                    let drop_after = Arc::clone(&drop_after);
                    let stall = Arc::clone(&stall_flag);
                    let corrupt = Arc::clone(&corrupt_flag);
                    let drops_left = Arc::clone(&drops_left);
                    let body = body.clone();
                    std::thread::spawn(move || {
                        let mut buf = Vec::new();
                        let mut byte = [0u8; 1];
                        loop {
                            use std::io::Read;
                            if conn.read(&mut byte).unwrap_or(0) == 0 {
                                return;
                            }
                            buf.push(byte[0]);
                            if buf.ends_with(b"\r\n\r\n") {
                                break;
                            }
                            if buf.len() > 64 * 1024 {
                                return;
                            }
                        }
                        let head = String::from_utf8_lossy(&buf);
                        let range = head
                            .lines()
                            .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                            .map(|l| l.trim().to_string());
                        let start = range
                            .as_deref()
                            .and_then(|r| r.strip_prefix("Range: bytes="))
                            .and_then(|r| r.split('-').next())
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(0);
                        reqs.lock().unwrap().push(range.unwrap_or_default());

                        let slice = &body[start.min(body.len())..];
                        let mut out: Vec<u8> = Vec::new();
                        if start > 0 {
                            out.extend_from_slice(
                                format!(
                                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\n\r\n",
                                    start,
                                    body.len() - 1,
                                    body.len(),
                                    slice.len()
                                )
                                .as_bytes(),
                            );
                        } else {
                            out.extend_from_slice(
                                format!(
                                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                                    body.len()
                                )
                                .as_bytes(),
                            );
                        }
                        if stall.load(Ordering::SeqCst) {
                            let _ = conn.write_all(&out);
                            std::thread::sleep(std::time::Duration::from_secs(30));
                            return;
                        }
                        let mut payload: Vec<u8> = slice.to_vec();
                        if corrupt.load(Ordering::SeqCst) && !payload.is_empty() {
                            payload[0] ^= 0xff; // sha must mismatch
                        }
                        // Honour the drop limit only while a drop is still
                        // pending — later connections serve the full body.
                        let limit = if drops_left.load(Ordering::SeqCst) > 0 {
                            drops_left.fetch_sub(1, Ordering::SeqCst);
                            drop_after.load(Ordering::SeqCst)
                        } else {
                            usize::MAX
                        };
                        let take = payload.len().min(limit);
                        out.extend_from_slice(&payload[..take]);
                        let _ = conn.write_all(&out);
                        // dropping `conn` mid-message simulates a dropped stream
                    });
                }
            });
            Self {
                addr,
                requests,
                stall: stall_flag,
            }
        }

        fn seen_ranges(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_completes_and_verifies_sha() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = UpdateConfig::default();
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let sha = {
            let mut h = Sha256::new();
            h.update(&body);
            hex(&h.finalize())
        };
        let srv = MockAsset::spawn(body);
        let dest = tmp.path().join("asset.bin");
        let got = download_asset_limits(
            &cfg,
            &format!("http://{}/asset", srv.addr),
            &dest,
            sha,
            300_000,
            &mut |_, _| {},
            DownloadLimits {
                idle_timeout: Duration::from_secs(2),
                attempts: 2,
            },
        )
        .await
        .unwrap();
        assert_eq!(got.len(), 64, "returns the sha256");
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), 300_000);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_resumes_after_dropped_connection() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = UpdateConfig::default();
        let body: Vec<u8> = (0..400_000u32).map(|i| (i % 253) as u8).collect();
        let sha = {
            let mut h = Sha256::new();
            h.update(&body);
            hex(&h.finalize())
        };
        // Attempt 1 dies after 50k bytes; the next attempt must send Range.
        let srv = MockAsset::spawn_with(body, 50_000, false, false);
        let dest = tmp.path().join("asset.bin");
        let mut progress_calls = 0usize;
        let got = download_asset_limits(
            &cfg,
            &format!("http://{}/asset", srv.addr),
            &dest,
            sha,
            400_000,
            &mut |_, _| progress_calls += 1,
            DownloadLimits {
                idle_timeout: Duration::from_secs(2),
                attempts: 3,
            },
        )
        .await
        .unwrap();
        assert_eq!(got.len(), 64);
        let ranges = srv.seen_ranges();
        assert!(
            ranges
                .iter()
                .any(|r| r.to_ascii_lowercase().starts_with("range: bytes=")),
            "a retry must RESUME via Range, saw: {ranges:?}"
        );
        assert!(progress_calls > 0, "progress callback must fire");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_survives_a_stalled_server_via_idle_timeout() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = UpdateConfig::default();
        let body = vec![7u8; 10_000];
        let sha = {
            let mut h = Sha256::new();
            h.update(&body);
            hex(&h.finalize())
        };
        let srv = MockAsset::spawn(body);
        srv.stall.store(true, std::sync::atomic::Ordering::SeqCst);
        let dest = tmp.path().join("asset.bin");
        let err = download_asset_limits(
            &cfg,
            &format!("http://{}/asset", srv.addr),
            &dest,
            sha,
            10_000,
            &mut |_, _| {},
            DownloadLimits {
                idle_timeout: Duration::from_millis(300),
                attempts: 2,
            },
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("stalled"),
            "expected stall error, got: {err}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_retries_fresh_on_checksum_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = UpdateConfig::default();
        let body = vec![9u8; 20_000];
        let sha = {
            let mut h = Sha256::new();
            h.update(&body);
            hex(&h.finalize())
        };
        let srv = MockAsset::spawn_with(body, usize::MAX, false, true); // corrupt
        let dest = tmp.path().join("asset.bin");
        let err = download_asset_limits(
            &cfg,
            &format!("http://{}/asset", srv.addr),
            &dest,
            sha,
            20_000,
            &mut |_, _| {},
            DownloadLimits {
                idle_timeout: Duration::from_secs(2),
                attempts: 2,
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, UpdateError::ChecksumMismatch { .. }),
            "corrupt body must surface as ChecksumMismatch, got: {err}"
        );
    }

    #[test]
    fn version_newer_handles_prerelease_chunks() {
        // The v0.4.6 parser read "7-test" as 0 — a prerelease of the NEXT
        // version looked "not newer". Leading digits fix that.
        assert!(version_newer("0.4.7-beta", "0.4.6"));
        assert!(version_newer("0.5.0-test", "0.4.6"));
        assert!(!version_newer("0.4.6", "0.4.6"));
        assert!(!version_newer("0.4.5-rc1", "0.4.6"));
    }

    #[cfg(unix)]
    #[test]
    fn helper_and_setup_scripts_have_the_right_shape() {
        // The privileged helper only ever touches whitelisted hyprfetch paths.
        assert!(HELPER_SH.contains("case \"$1\" in"));
        assert!(HELPER_SH.contains("check)"));
        assert!(HELPER_SH.contains("install)"));
        assert!(HELPER_SH.contains("remove)"));
        assert!(HELPER_SH.contains("/usr/bin/hyprfetch|/usr/local/bin/hyprfetch"));
        assert!(!HELPER_SH.contains("eval"));

        let script = setup_script(
            "alice",
            Path::new("/tmp/hyprfetch-update-x/hyprfetch"),
            Path::new("/usr/bin/hyprfetch"),
        );
        assert!(
            script.contains("alice ALL=(root) NOPASSWD: /usr/lib/hyprfetch/privileged-update *")
        );
        assert!(script.contains("visudo -cf"), "sudoers must be validated");
        assert!(script.contains("install -m 0440"));
        // finishes the pending swap with the staged binary
        assert!(script.contains("install '/tmp/hyprfetch-update-x/hyprfetch' '/usr/bin/hyprfetch'"));
        // the helper heredoc is quoted so nothing expands inside it
        assert!(script.contains("<<'HYPRFETCH_HELPER_EOF'"));
    }

    #[cfg(unix)]
    #[test]
    fn terminal_candidate_builds_valid_command_lines() {
        let _guard = PATH_LOCK.blocking_lock();
        let old_path = std::env::var_os("PATH").unwrap();
        let tmp = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        let kitty = tmp.path().join("kitty");
        std::fs::write(&kitty, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&kitty, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("PATH", tmp.path().display().to_string());
        let (program, args) = terminal_candidate("echo hi").unwrap();
        std::env::set_var("PATH", old_path.clone());
        assert_eq!(program, "kitty");
        assert_eq!(&args[..2], &["-e".to_string(), "sh".to_string()]);
        assert_eq!(args[2], "-c");
        assert_eq!(args[3], "echo hi");

        // no terminal on PATH → None (the caller shows the manual command)
        let empty = tempfile::tempdir().unwrap();
        std::env::set_var("PATH", empty.path().display().to_string());
        assert!(terminal_candidate("echo hi").is_none());
        std::env::set_var("PATH", old_path);
    }

    #[test]
    fn migrate_moves_binary_and_keeps_rollback() {
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join(".local").join("bin");
        let staged = tmp.path().join("staged-bin");
        std::fs::write(&staged, b"NEWBIN").unwrap();

        // First migration: no previous binary → no backup.
        let m = migrate_to_user_bin_into(&bin_dir, &staged).unwrap();
        assert_eq!(m.new_path, bin_dir.join("hyprfetch"));
        assert_eq!(std::fs::read(&m.new_path).unwrap(), b"NEWBIN");
        assert!(m.backup_path.is_none());
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&m.new_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "installed with the usual mode");

        // Second migration over an existing binary keeps the .old rollback.
        std::fs::write(&staged, b"NEWBIN2").unwrap();
        let m2 = migrate_to_user_bin_into(&bin_dir, &staged).unwrap();
        assert_eq!(
            std::fs::read(m2.backup_path.as_ref().unwrap()).unwrap(),
            b"NEWBIN",
            "previous binary kept as ~/.local/bin/hyprfetch.old"
        );
        assert_eq!(std::fs::read(&m2.new_path).unwrap(), b"NEWBIN2");
        assert!(!bin_dir.join("hyprfetch.new").exists());
    }

    #[test]
    fn ensure_user_path_writes_guarded_blocks_once() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".profile"), b"export FOO=bar\n").unwrap();
        let bin_dir = home.path().join(".local").join("bin");

        let touched = ensure_user_path_under(home.path(), &bin_dir);
        assert!(
            touched.iter().any(|t| t.contains(".profile")),
            "profile reported as touched: {touched:?}"
        );
        for name in [".profile", ".bashrc", ".zshrc"] {
            let content = std::fs::read_to_string(home.path().join(name)).unwrap();
            assert!(
                content.contains("# >>> hyprfetch PATH >>>"),
                "{name}: {content}"
            );
            assert!(content.contains("export PATH="), "{name}: {content}");
            assert!(content.contains(".local/bin"), "{name}: {content}");
        }
        // Pre-existing content survives.
        let profile = std::fs::read_to_string(home.path().join(".profile")).unwrap();
        assert!(profile.starts_with("export FOO=bar\n"));

        // Idempotent: a second run changes nothing (files already cover it).
        let touched2 = ensure_user_path_under(home.path(), &bin_dir);
        assert!(
            touched2.iter().all(|t| !t.ends_with(".profile")
                && !t.ends_with(".bashrc")
                && !t.ends_with(".zshrc")),
            "rc files must not be touched twice: {touched2:?}"
        );
    }

    #[test]
    fn link_relinks_writable_system_copy_directly() {
        let tmp = tempfile::tempdir().unwrap();
        let sys_dir = tmp.path().join("system");
        std::fs::create_dir_all(&sys_dir).unwrap();
        let sys = sys_dir.join("hyprfetch");
        std::fs::write(&sys, b"OLD").unwrap();
        std::fs::write(sys_dir.join("hyprfetch.old"), b"OLDER").unwrap();
        let user = tmp.path().join("user").join("hyprfetch");
        std::fs::create_dir_all(user.parent().unwrap()).unwrap();
        std::fs::write(&user, b"NEW").unwrap();

        let fix = link_system_copies_to_user(std::slice::from_ref(&sys), &user, false);
        let via = match &fix {
            SystemCopyFix::Linked { via, linked } => {
                assert_eq!(linked.len(), 1, "{linked:?}");
                via.clone()
            }
            other => panic!("expected Linked, got {other:?}"),
        };
        let md = std::fs::symlink_metadata(&sys).unwrap();
        assert!(md.file_type().is_symlink(), "system copy is now a symlink");
        assert_eq!(
            std::fs::canonicalize(&sys).unwrap(),
            std::fs::canonicalize(&user).unwrap()
        );
        assert!(
            !sys.with_extension("old").exists(),
            "stale .old backups are cleaned by the relink"
        );
        assert!(!via.is_empty());

        // Second call: everything already points at the user binary.
        let fix2 = link_system_copies_to_user(std::slice::from_ref(&sys), &user, false);
        assert_eq!(fix2, SystemCopyFix::AlreadyLinked);

        // No system copy at all → NotNeeded.
        let missing = tmp.path().join("nowhere").join("hyprfetch");
        let fix3 = link_system_copies_to_user(std::slice::from_ref(&missing), &user, false);
        assert_eq!(fix3, SystemCopyFix::NotNeeded);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn link_reports_one_liner_when_unwritable_and_no_priv_tool() {
        let _guard = PATH_LOCK.lock().await;
        let old_path = std::env::var_os("PATH").unwrap();
        let tmp = tempfile::tempdir().unwrap();

        // PATH with NO sudo and NO pkexec → every privilege rung declines.
        let empty = tempfile::tempdir().unwrap();
        std::env::set_var("PATH", empty.path().display().to_string());

        let sys_dir = tmp.path().join("system");
        std::fs::create_dir_all(&sys_dir).unwrap();
        let sys = sys_dir.join("hyprfetch");
        std::fs::write(&sys, b"OLD").unwrap();
        let user = tmp.path().join("user").join("hyprfetch");
        std::fs::create_dir_all(user.parent().unwrap()).unwrap();
        std::fs::write(&user, b"NEW").unwrap();

        // A read-only parent dir blocks the direct route too.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&sys_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

        let fix = link_system_copies_to_user(std::slice::from_ref(&sys), &user, false);

        // Restore BEFORE asserting so a panic can never leave bad state.
        std::fs::set_permissions(&sys_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("PATH", old_path);

        match fix {
            SystemCopyFix::Failed { path, hint } => {
                assert_eq!(path, sys.to_string_lossy());
                assert!(hint.starts_with("sudo rm -f"), "hint: {hint}");
                assert!(hint.contains("ln -sf"), "hint: {hint}");
                assert!(
                    hint.contains(user.to_string_lossy().as_ref()),
                    "hint: {hint}"
                );
                // The copy must be UNTOUCHED (self-recovering failure).
                assert!(std::fs::read(&sys).unwrap() == b"OLD");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn link_uses_passwordless_sudo_shim_when_available() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = PATH_LOCK.lock().await;
        let old_path = std::env::var_os("PATH").unwrap();
        let tmp = tempfile::tempdir().unwrap();

        // A sudo SHIM that records its arguments and exits 0 — the sandbox
        // has no real root, so the assertion is about rung selection and
        // the script handed to sudo (the real linking is covered by the
        // direct-route test and the battery script).
        let spy = tmp.path().join("spy.log");
        let shim_dir = tempfile::tempdir().unwrap();
        let shim = shim_dir.path().join("sudo");
        std::fs::write(
            &shim,
            format!("#!/bin/sh\necho \"$@\" >> '{}'\nexit 0\n", spy.display()),
        )
        .unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("PATH", shim_dir.path().display().to_string());

        // NOTE: parent dir NOT writable → the direct route is skipped and
        // the sudo rung must be picked.
        let sys_dir_ro = tempfile::tempdir().unwrap();
        let sys = sys_dir_ro.path().join("hyprfetch");
        std::fs::write(&sys, b"OLD").unwrap();
        std::fs::set_permissions(sys_dir_ro.path(), std::fs::Permissions::from_mode(0o555))
            .unwrap();

        let user = tmp.path().join("user").join("hyprfetch");
        std::fs::create_dir_all(user.parent().unwrap()).unwrap();
        std::fs::write(&user, b"NEW").unwrap();

        let fix = link_system_copies_to_user(std::slice::from_ref(&sys), &user, false);

        let spy_contents = std::fs::read_to_string(&spy).unwrap_or_default();
        std::fs::set_permissions(sys_dir_ro.path(), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        std::env::set_var("PATH", old_path);

        match fix {
            SystemCopyFix::Linked { via, .. } => assert_eq!(via, "sudo", "sudo rung chosen"),
            other => panic!("expected Linked via sudo shim, got {other:?}"),
        }
        // The shim must have received the full rm+ln script for BOTH paths.
        assert!(spy_contents.contains("/bin/sh"), "spy: {spy_contents}");
        assert!(spy_contents.contains("-c"), "spy: {spy_contents}");
        assert!(spy_contents.contains("rm -f"), "spy: {spy_contents}");
        assert!(spy_contents.contains("ln -sf"), "spy: {spy_contents}");
        assert!(
            spy_contents.contains(user.to_string_lossy().as_ref()),
            "spy: {spy_contents}"
        );
    }
}
