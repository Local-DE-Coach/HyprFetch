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
/// Per-request timeout for update HTTP calls.
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);

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
    #[error("{0}")]
    Other(String),
}

/// What the updater may do when the running binary sits in a root-owned
/// directory (e.g. `/usr/bin` when installed via pacman/makepkg or .deb/.rpm).
///
/// A plain user process cannot write there, so the atomic swap would fail
/// with "permission denied". With [`Escalation::Auto`] the updater performs
/// the swap through a privilege tool (`sudo`, falling back to `doas`); with
/// [`Escalation::Refuse`] it returns [`UpdateError::RootNeeded`] with a
/// actionable hint instead (used by the non-interactive web UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escalation {
    /// Replace system-owned binaries via `sudo`/`doas` (sudo may prompt for a
    /// password — this is the interactive CLI choice).
    Auto,
    /// Never escalate; fail with [`UpdateError::RootNeeded`] instead (WebUI).
    Refuse,
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
    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
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

/// Compare dotted numeric versions (`0.3.1` > `0.3.0`); non-numeric chunks
/// compare lexicographically as fallback.
pub fn version_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<(u64, String)> {
        v.trim()
            .trim_start_matches('v')
            .split('.')
            .map(|c| (c.parse::<u64>().unwrap_or(0), c.to_string()))
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

/// Install: fetch the manifest, download the archive for this target,
/// sha256-verify against the manifest, extract + atomic swap.
///
/// `escalation` decides how a system-owned install location is handled (see
/// [`Escalation`]). The writability probe runs BEFORE the download, so a
/// system install without escalation rights fails fast with a clear hint
/// instead of after pulling the whole archive.
pub async fn apply(
    cfg: &UpdateConfig,
    chk: &UpdateCheck,
    escalation: Escalation,
) -> Result<ApplyResult, UpdateError> {
    // Resolve the running binary + pick the swap strategy up front (fail
    // fast before spending time on the download).
    let exe = std::env::current_exe().map_err(|_| UpdateError::ExePath)?;
    let escalate_with: Option<Option<&'static str>> = if can_swap_in_place(&exe) {
        Some(None) // direct swap, no privileges needed
    } else {
        match escalation {
            Escalation::Auto => Some(Some(
                find_priv_tool().ok_or_else(|| root_needed(&exe, true))?,
            )),
            Escalation::Refuse => return Err(root_needed(&exe, false)),
        }
    };

    let m = fetch_manifest(cfg).await?;
    let (_, asset) = pick_channel_asset(&m.assets)
        .ok_or_else(|| UpdateError::AssetMissing("target tarball (update channel)".into()))?;

    let tarball = cfg
        .client()
        .get(&asset.url)
        .send()
        .await
        .map_err(|e| UpdateError::Channel(format!("asset get: {e}")))?
        .error_for_status()
        .map_err(|e| UpdateError::Channel(format!("asset get: {e}")))?
        .bytes()
        .await
        .map_err(|e| UpdateError::Channel(format!("asset read: {e}")))?
        .to_vec();

    let expected = asset.sha256.trim().to_ascii_lowercase();
    let got = sha256_hex(&tarball);
    if got != expected {
        return Err(UpdateError::ChecksumMismatch { expected, got });
    }

    let new_bytes = extract_binary(&tarball)?;
    let escalated = match escalate_with {
        Some(None) | None => {
            swap_binary(&exe, &new_bytes)?;
            false
        }
        // can_swap_in_place() is only ever false on unix; on other targets
        // the arm is unreachable, but keep the code honest anyway.
        Some(Some(tool)) => {
            swap_binary_escalated(&exe, &new_bytes, tool)?;
            true
        }
    };

    Ok(ApplyResult {
        current: chk.current.clone(),
        installed: chk.latest.clone(),
        backup_path: Some(exe.with_extension("old").to_string_lossy().into_owned()),
        sha256: got,
        escalated,
    })
}

/// Build the [`UpdateError::RootNeeded`] error with an actionable hint.
fn root_needed(exe: &Path, no_tool: bool) -> UpdateError {
    let path = exe.to_string_lossy().into_owned();
    let hint = if no_tool {
        "install sudo or doas, then run `sudo hyprfetch update` — or reinstall \
         with the one-line installer (curl -fsSL https://istias.tech/hyprfetch/updates/install.sh | sh)"
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
        .map(|rest| rest.trim().trim_end_matches('.') .to_string())
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

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex(&h.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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
    /// True when the swap needed sudo/doas (system-owned install location).
    pub escalated: bool,
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
    tool: &str,
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

    let run = std::process::Command::new(tool)
        .args(["sh", "-c", &script])
        .status();
    // Best-effort staging cleanup (the script removes the staged file on
    // success; the dir may still be left over on failure).
    let _ = std::fs::remove_dir_all(&stage_dir);

    match run {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(UpdateError::Other(format!(
            "privileged swap via {tool} failed (exit {status}) — {path} was left untouched; \
             run `sudo hyprfetch update` manually if the problem persists",
            path = exe.display(),
        ))),
        Err(e) => Err(UpdateError::Other(format!(
            "cannot run {tool}: {e} — install sudo/doas or run `sudo hyprfetch update` manually"
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

        swap_binary_escalated(&exe, b"new-binary", shim.to_str().unwrap()).unwrap();

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
            .filter(|e| e.file_name().to_string_lossy().starts_with("hyprfetch-update-"))
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

        let err =
            swap_binary_escalated(&exe, b"new-binary", shim.to_str().unwrap()).unwrap_err();
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
        assert!(!exe.with_extension("old").exists(), "rollback restored the original");
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
}
