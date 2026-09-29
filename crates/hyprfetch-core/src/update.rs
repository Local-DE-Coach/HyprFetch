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
use std::path::Path;
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
    #[error("{0}")]
    Other(String),
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
pub async fn apply(cfg: &UpdateConfig, chk: &UpdateCheck) -> Result<ApplyResult, UpdateError> {
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
    let exe = std::env::current_exe().map_err(|_| UpdateError::ExePath)?;
    swap_binary(&exe, &new_bytes)?;

    Ok(ApplyResult {
        current: chk.current.clone(),
        installed: chk.latest.clone(),
        backup_path: Some(exe.with_extension("old").to_string_lossy().into_owned()),
        sha256: got,
    })
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
