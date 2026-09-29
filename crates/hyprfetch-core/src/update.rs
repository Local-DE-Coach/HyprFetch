//! Self-update: GitHub-release check + sha256-verified binary swap.
//!
//! Works against public AND private repos (fine-grained PAT via
//! `Authorization: Bearer`). Private-repo assets MUST be downloaded through
//! the REST API endpoint with `Accept: application/octet-stream` — the
//! `github.com/.../releases/download/...` browser URL answers 404 for PATs
//! it cannot associate with a web session.
//!
//! The API base is overridable (`HYPRFETCH_UPDATE_API`) so tests can run
//! against a local mock release server.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Default GitHub repo checked by the updater.
pub const DEFAULT_REPO: &str = "Local-DE-Coach/HyprFetch";
/// GitHub REST API base.
pub const DEFAULT_API_BASE: &str = "https://api.github.com";
/// Per-request timeout for update HTTP calls.
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);

/// Errors returned by the updater.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("repo setting is not `owner/name`: {0}")]
    BadRepo(String),
    #[error("asset {0} not found in release")]
    AssetMissing(String),
    #[error("sha256 mismatch: expected {expected}, got {got}")]
    ChecksumMismatch { expected: String, got: String },
    #[error("archive does not contain a `hyprfetch` binary")]
    BinaryMissing,
    #[error("cannot determine current executable path")]
    ExePath,
    #[error("git: {0}")]
    Git(String),
    #[error("{0}")]
    Other(String),
}

/// Where/how to check for updates.
#[derive(Debug, Clone)]
pub struct UpdateConfig {
    /// `owner/name` on GitHub.
    pub repo: String,
    /// PAT for private repos (sent as `Authorization: Bearer`).
    pub token: Option<String>,
    /// Where the token came from, for UX labels (`"env"`, `"gh cli"`, …).
    pub token_source: Option<&'static str>,
    /// Git remote used by the git tier when the REST API cannot see the
    /// repo (private repo accessed via SSH keys / a local clone).
    /// Auto-discovered from a local clone's origin or set via
    /// `[update] git_url`; when absent it is derived from `repo`.
    pub git_url: Option<String>,
    /// API base override (tests / GHES).
    pub api_base: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            repo: DEFAULT_REPO.to_string(),
            token: None,
            token_source: None,
            git_url: None,
            api_base: DEFAULT_API_BASE.to_string(),
        }
    }
}

impl UpdateConfig {
    /// Validate the repo string is `owner/name`.
    pub fn validate(&self) -> Result<(), UpdateError> {
        let mut parts = self.repo.split('/');
        match (parts.next(), parts.next(), parts.next()) {
            (Some(o), Some(n), None) if !o.is_empty() && !n.is_empty() => Ok(()),
            _ => Err(UpdateError::BadRepo(self.repo.clone())),
        }
    }

    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(concat!("hyprfetch/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client")
    }

    fn api(&self, path: &str) -> String {
        format!("{}/{}", self.api_base.trim_end_matches('/'), path)
    }

    fn auth_headers(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let rb = rb.header("X-GitHub-Api-Version", "2022-11-28");
        match &self.token {
            Some(t) if !t.trim().is_empty() => rb.bearer_auth(t.trim()),
            _ => rb,
        }
    }
}

/// Metadata about the matching release asset.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AssetInfo {
    pub name: String,
    pub size: u64,
    pub id: u64,
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
    /// The tarball asset matching this machine's target triple, when present.
    /// `None` on the git tier — there the update builds from source instead.
    pub asset: Option<AssetInfo>,
    /// True when the check succeeded via plain git instead of the REST API
    /// (private repo + SSH/clone access, no token needed).
    #[serde(default)]
    pub via_git: bool,
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
// Git tier — private-repo updates WITHOUT a token
//
// SSH keys cannot call the GitHub REST API, but they can run git. So when
// the API tier comes back empty (private repo, no token), the updater falls
// back to plain git: `ls-remote --tags` finds the newest release tag with
// the user's existing credentials (SSH agent, credential helpers, or a
// local clone's remote), and the install step shallow-clones that tag and
// builds it with `cargo build --release --locked`.
// ---------------------------------------------------------------------------

/// Batch-mode SSH options so git never blocks on a passphrase/host-key
/// prompt — an unattended updater must fail fast, not hang.
const GIT_SSH_COMMAND: &str =
    "ssh -oBatchMode=yes -oConnectTimeout=10 -oStrictHostKeyChecking=accept-new";

/// A `git` invocation that can never stop to ask questions (no terminal
/// prompt, no interactive askpass, no interactive ssh).
fn git_cmd(args: &[&str]) -> std::process::Command {
    let mut c = std::process::Command::new("git");
    c.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "true")
        .env("GIT_SSH_COMMAND", GIT_SSH_COMMAND);
    c
}

/// Git remote URLs tried by the git tier, most-specific first:
/// 1. a configured/discovered remote (`[update] git_url` or a local clone's
///    origin — honours the user's SSH keys and credential helpers),
/// 2. the SSH URL derived from the repo slug,
/// 3. anonymous HTTPS (public fallback; the API tier already covers this).
pub fn git_url_candidates(cfg: &UpdateConfig) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(u) = cfg
        .git_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        out.push(u.to_string());
    }
    let ssh = format!("git@github.com:{}.git", cfg.repo);
    if !out.contains(&ssh) {
        out.push(ssh);
    }
    let https = format!("https://github.com/{}.git", cfg.repo);
    if !out.contains(&https) {
        out.push(https);
    }
    out
}

/// Parse `git ls-remote --tags` output into raw tag names (`v0.3.1`),
/// dropping peeled `^{}` refs, non-version tags and duplicates.
pub fn parse_version_tags(ls_remote_output: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in ls_remote_output.lines() {
        let mut it = line.split_whitespace();
        let (_, r) = match (it.next(), it.next()) {
            (Some(a), Some(b)) => (a, b),
            _ => continue,
        };
        let Some(tag) = r.strip_prefix("refs/tags/") else {
            continue;
        };
        let tag = tag.strip_suffix("^{}").unwrap_or(tag);
        let ver = tag.strip_prefix('v').unwrap_or(tag);
        let is_version = ver.chars().next().is_some_and(|c| c.is_ascii_digit())
            && ver.split('.').count() >= 2
            && ver.split('.').all(|c| !c.is_empty());
        if is_version && !out.iter().any(|t| t == tag) {
            out.push(tag.to_string());
        }
    }
    out
}

/// Newest tag among raw version tags (`v0.3.1`); `None` when empty.
pub fn latest_version_tag(tags: &[String]) -> Option<String> {
    tags.iter()
        .fold(None, |best: Option<String>, t| match best {
            Some(b) if !version_newer(t, &b) => Some(b),
            _ => Some(t.clone()),
        })
}

/// Blocking: newest version tag reachable at `url`. `Ok(None)` when the
/// remote answers but carries no version tags; `Err` when git fails
/// (unreachable / unauthorized).
pub fn ls_remote_latest_tag(url: &str) -> Result<Option<String>, UpdateError> {
    let out = git_cmd(&["ls-remote", "--tags", url])
        .output()
        .map_err(|e| UpdateError::Git(format!("git ls-remote spawn: {e}")))?;
    if !out.status.success() {
        return Err(UpdateError::Git(format!(
            "ls-remote {url}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let tags = parse_version_tags(&String::from_utf8_lossy(&out.stdout));
    Ok(latest_version_tag(&tags))
}

/// Outcome of a git-tier update check.
#[derive(Debug, Clone)]
pub struct GitCheck {
    /// The remote URL that answered (shown to the user as access proof).
    pub via_url: String,
    /// Raw tag of the newest release (`v0.3.1`) — used for `--branch`.
    pub tag: String,
    /// Standard check payload (`asset` is always None — git builds from
    /// source; `via_git` is true).
    pub check: UpdateCheck,
}

/// API-independent update check for private repos: reads version tags over
/// plain git using whatever credentials the user already has (SSH agent,
/// credential helpers, clone remotes). Blocking — prefer [`check_via_git`]
/// from async contexts.
pub fn check_via_git_sync(cfg: &UpdateConfig) -> Result<Option<GitCheck>, UpdateError> {
    cfg.validate()?;
    let current = env!("CARGO_PKG_VERSION").to_string();
    let mut last_err: Option<UpdateError> = None;
    for url in git_url_candidates(cfg) {
        match ls_remote_latest_tag(&url) {
            Ok(Some(tag)) => {
                let latest = tag.trim_start_matches('v').to_string();
                return Ok(Some(GitCheck {
                    via_url: url,
                    tag,
                    check: UpdateCheck {
                        available: version_newer(&latest, &current),
                        current,
                        latest,
                        published_at: None,
                        release_url: Some(format!("https://github.com/{}/releases", cfg.repo)),
                        asset: None,
                        via_git: true,
                    },
                }));
            }
            // Reachable but no version tags: same as "no published release".
            Ok(None) => return Ok(None),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or(UpdateError::Other("no git remote candidate".into())))
}

/// Async wrapper around [`check_via_git_sync`] (git spawns are blocking).
pub async fn check_via_git(cfg: &UpdateConfig) -> Result<Option<GitCheck>, UpdateError> {
    let cfg = cfg.clone();
    tokio::task::spawn_blocking(move || check_via_git_sync(&cfg))
        .await
        .map_err(|e| UpdateError::Other(format!("git check join: {e}")))?
}

/// Blocking: shallow-clone `url` at `tag` into `dest` (created, emptied).
pub fn clone_tag_shallow(url: &str, tag: &str, dest: &Path) -> Result<(), UpdateError> {
    let _ = std::fs::remove_dir_all(dest);
    std::fs::create_dir_all(dest)?;
    let out = git_cmd(&[
        "clone",
        "--depth",
        "1",
        "--branch",
        tag,
        "--single-branch",
        url,
    ])
    .arg(dest)
    .output()
    .map_err(|e| UpdateError::Git(format!("git clone spawn: {e}")))?;
    if !out.status.success() {
        return Err(UpdateError::Git(format!(
            "clone {tag}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Locate a usable `cargo`: PATH first, then the rustup default location.
pub fn find_cargo() -> Result<String, UpdateError> {
    let usable = |p: &str| {
        std::process::Command::new(p)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    if usable("cargo") {
        return Ok("cargo".into());
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = format!("{home}/.cargo/bin/cargo");
        if usable(&p) {
            return Ok(p);
        }
    }
    Err(UpdateError::Other(
        "cargo not found on PATH — source updates need the Rust toolchain \
         (https://rustup.rs), or install a release tarball instead"
            .into(),
    ))
}

/// `cargo build --release --locked` inside `dir`. Blocking; on failure the
/// error keeps the last 15 stderr lines (enough to see the failing crate).
pub fn cargo_build_release(dir: &Path) -> Result<(), UpdateError> {
    let cargo = find_cargo()?;
    let out = std::process::Command::new(&cargo)
        .args(["build", "--release", "--locked"])
        .current_dir(dir)
        .output()
        .map_err(|e| UpdateError::Git(format!("cargo spawn: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let mut tail: Vec<&str> = stderr.lines().rev().take(15).collect();
        tail.reverse();
        return Err(UpdateError::Git(format!(
            "cargo build failed: {}",
            tail.join("\n")
        )));
    }
    Ok(())
}

/// Full git-tier install: shallow-clone `url` at `tag` into `dest`, build a
/// release binary, return its path. Blocking and long-running (a cold build
/// takes minutes).
pub fn build_from_tag(url: &str, tag: &str, dest: &Path) -> Result<PathBuf, UpdateError> {
    clone_tag_shallow(url, tag, dest)?;
    cargo_build_release(dest)?;
    Ok(dest.join("target/release/hyprfetch"))
}

/// Parse a PAT out of a git remote URL. Clone-based installs carry the
/// token right in the origin URL (`https://<PAT>@github.com/…`), which lets
/// `hyprfetch update` work with zero extra configuration.
///
/// Accepted shapes:
/// - `https://<token>@github.com/owner/repo.git`
/// - `https://x-access-token:<token>@github.com/…`
/// - `https://<any-user>:<token>@github.com/…` (GitHub accepts any username
///   with a PAT — e.g. `https://myuser:<PAT>@github.com/…`)
///
/// Anything else (SSH remotes, token-less URLs) returns `None`.
pub fn parse_pat_from_git_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let (creds, host) = rest.split_once('@')?;
    if !host.starts_with("github.com") {
        return None;
    }
    let token = match creds.split_once(':') {
        // user:pass form — the password part is the token.
        Some((_user, pass)) if !pass.is_empty() => pass,
        Some(_) => return None,
        None => creds,
    };
    let token = token.trim();
    if token.len() < 20 {
        return None; // too short to be a PAT — avoid picking up junk
    }
    Some(token.to_string())
}

/// The release tarball naming scheme is `hyprfetch-<ver>-<target>.tar.gz`.
/// Derive the target triple candidates for the running binary (Linux-first).
fn target_candidates() -> Vec<String> {
    let arch = std::env::consts::ARCH;
    vec![
        format!("{arch}-unknown-linux-gnu"),
        format!("{arch}-unknown-linux-musl"),
    ]
}

/// Find the tarball asset matching this machine among all release assets.
pub fn pick_asset(assets: &[serde_json::Value], version: &str) -> Option<AssetInfo> {
    for target in target_candidates() {
        let want = format!("hyprfetch-{version}-{target}.tar.gz");
        for a in assets {
            let name = a.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            if name == want {
                return Some(AssetInfo {
                    name: name.to_string(),
                    size: a.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
                    id: a.get("id").and_then(|v| v.as_u64()).unwrap_or(0),
                });
            }
        }
    }
    None
}

/// Query the latest published release. `Ok(None)` when the repo has none.
pub async fn check(cfg: &UpdateConfig) -> Result<Option<UpdateCheck>, UpdateError> {
    cfg.validate()?;
    let current = env!("CARGO_PKG_VERSION").to_string();

    let rb = cfg
        .client()
        .get(cfg.api(&format!("repos/{}/releases/latest", cfg.repo)));
    let resp = cfg.auth_headers(rb).send().await?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let body: serde_json::Value = resp.error_for_status()?.json().await?;

    let tag = body
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let latest = tag.trim_start_matches('v').to_string();
    let assets = body
        .get("assets")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(Some(UpdateCheck {
        available: version_newer(&latest, &current),
        current,
        latest: latest.clone(),
        published_at: body
            .get("published_at")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        release_url: body
            .get("html_url")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        asset: pick_asset(&assets, &latest),
        via_git: false,
    }))
}

/// Download one release asset's bytes through the API octet-stream endpoint.
async fn download_asset(cfg: &UpdateConfig, asset: &AssetInfo) -> Result<Vec<u8>, UpdateError> {
    let rb = cfg
        .client()
        .get(cfg.api(&format!("repos/{}/releases/assets/{}", cfg.repo, asset.id)));
    let resp = cfg
        .auth_headers(rb)
        .header("Accept", "application/octet-stream")
        .send()
        .await?
        .error_for_status()?;
    Ok(resp.bytes().await?.to_vec())
}

/// Parse a `.sha256` file body (`<hash>  <filename>`), returning the hash.
pub fn parse_sha256_file(body: &str) -> Option<String> {
    body.split_whitespace().next().map(str::to_ascii_lowercase)
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
pub async fn apply(cfg: &UpdateConfig, chk: &UpdateCheck) -> Result<ApplyResult, UpdateError> {
    cfg.validate()?;

    // 1. Fetch the release listing once and locate BOTH assets we need.
    let rb = cfg
        .client()
        .get(cfg.api(&format!("repos/{}/releases/latest", cfg.repo)));
    let body: serde_json::Value = cfg
        .auth_headers(rb)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let assets = body
        .get("assets")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let asset = pick_asset(&assets, &chk.latest)
        .ok_or_else(|| UpdateError::AssetMissing("target tarball".into()))?;
    let sha_asset = {
        let want = format!("{}.sha256", asset.name);
        assets
            .iter()
            .find(|a| a.get("name").and_then(|v| v.as_str()) == Some(want.as_str()))
            .map(|a| AssetInfo {
                name: want.clone(),
                size: a.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
                id: a.get("id").and_then(|v| v.as_u64()).unwrap_or(0),
            })
            .ok_or_else(|| UpdateError::AssetMissing(want))?
    };

    // 2. Download tarball + checksum asset.
    let tarball = download_asset(cfg, &asset).await?;
    let sha_body = download_asset(cfg, &sha_asset).await?;
    let expected = parse_sha256_file(std::str::from_utf8(&sha_body).unwrap_or(""))
        .ok_or_else(|| UpdateError::Other("unparseable .sha256 asset".into()))?;

    // 3. Verify checksum.
    let got = sha256_hex(&tarball);
    if got != expected {
        return Err(UpdateError::ChecksumMismatch { expected, got });
    }

    // 4. Extract the binary.
    let new_bytes = extract_binary(&tarball)?;

    // 5. Atomic swap next to the current exe.
    let exe = std::env::current_exe().map_err(|_| UpdateError::ExePath)?;
    swap_binary(&exe, &new_bytes)?;

    Ok(ApplyResult {
        current: chk.current.clone(),
        installed: chk.latest.clone(),
        backup_path: Some(exe.with_extension("old").to_string_lossy().into_owned()),
        sha256: got,
    })
}

/// Filesystem part of the swap, split out for unit testing.
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

/// `git pull --ff-only` + `cargo build --release --locked` inside an
/// existing source clone (the explicit `--from-git` path).
pub fn run_git_update(source_dir: &Path) -> Result<String, UpdateError> {
    let run = |args: &[&str]| -> Result<String, UpdateError> {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(source_dir)
            .output()
            .map_err(|e| UpdateError::Git(format!("git spawn: {e}")))?;
        if !out.status.success() {
            return Err(UpdateError::Git(format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    run(&["pull", "--ff-only"])?;
    cargo_build_release(source_dir)?;
    Ok(source_dir
        .join("target/release/hyprfetch")
        .to_string_lossy()
        .into_owned())
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
    fn asset_pick_matches_target() {
        let mk = |name: &str| serde_json::json!({"name": name, "size": 10, "id": 1});
        let assets = vec![
            mk("hyprfetch-0.3.1-aarch64-unknown-linux-gnu.tar.gz"),
            mk("hyprfetch-0.3.1-x86_64-unknown-linux-gnu.tar.gz"),
            mk("hyprfetch-0.3.1-x86_64-unknown-linux-musl.tar.gz"),
        ];
        // On this machine pick_asset matches ARCH — verify it picks SOMETHING
        // and that a wrong-version list yields None.
        let picked = pick_asset(&assets, "0.3.1");
        assert!(picked.is_some(), "should find an asset for the host arch");
        assert!(pick_asset(&assets, "9.9.9").is_none());
    }

    #[test]
    fn sha256_file_parsing() {
        assert_eq!(
            parse_sha256_file("abc123  hyprfetch-0.3.1.tar.gz\n").unwrap(),
            "abc123"
        );
        assert_eq!(parse_sha256_file("ABCDEF00  x").unwrap(), "abcdef00");
        assert!(parse_sha256_file("").is_none());
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
            std::fs::read(tmp.path().join("hyprfetch.old")).unwrap(),
            b"old-binary"
        );
        assert!(!tmp.path().join("hyprfetch.new").exists());
    }

    #[test]
    fn config_validation() {
        let mut cfg = UpdateConfig::default();
        assert!(cfg.validate().is_ok());
        cfg.repo = "just-a-name".into();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn version_tag_parsing() {
        let out = "abc123\trefs/tags/v0.2.0\n\
                   abc124\trefs/tags/v0.3.1\n\
                   abc125\trefs/tags/v0.3.1^{}\n\
                   abc126\trefs/tags/nightly\n\
                   abc127\trefs/tags/vX.Y\n\
                   abc128\trefs/tags/1.9\n";
        assert_eq!(parse_version_tags(out), vec!["v0.2.0", "v0.3.1", "1.9"]);
        assert!(parse_version_tags("").is_empty());
        // Bare line without a ref is ignored.
        assert!(parse_version_tags("abc123\n").is_empty());
    }

    #[test]
    fn latest_tag_selection() {
        let tags: Vec<String> = ["v0.2.0", "v0.3.1", "v1.0.0-rc1", "0.9.9"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(latest_version_tag(&tags).as_deref(), Some("v1.0.0-rc1"));
        let tags: Vec<String> = ["v0.3.1", "v0.2.0"].iter().map(|s| s.to_string()).collect();
        assert_eq!(latest_version_tag(&tags).as_deref(), Some("v0.3.1"));
        assert!(latest_version_tag(&[]).is_none());
    }

    #[test]
    fn git_candidate_ordering() {
        let mut cfg = UpdateConfig::default();
        assert_eq!(
            git_url_candidates(&cfg),
            vec![
                "git@github.com:Local-DE-Coach/HyprFetch.git".to_string(),
                "https://github.com/Local-DE-Coach/HyprFetch.git".to_string(),
            ]
        );
        cfg.git_url = Some(" file:///tmp/hf.git ".into());
        assert_eq!(git_url_candidates(&cfg)[0], "file:///tmp/hf.git");
        // A candidate equal to the derived SSH URL is not repeated.
        cfg.git_url = Some("git@github.com:Local-DE-Coach/HyprFetch.git".into());
        assert_eq!(git_url_candidates(&cfg).len(), 2);
    }

    /// Minimal local repo (file:// remote) for the real-subprocess tests.
    fn init_test_repo(dir: &Path, tags: &[&str]) -> String {
        std::fs::create_dir_all(dir).unwrap();
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-q", "."]);
        std::fs::write(dir.join("f.txt"), "x").unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "init"]);
        for t in tags {
            run(&["tag", t]);
        }
        format!("file://{}", dir.display())
    }

    #[test]
    fn ls_remote_and_shallow_clone_on_local_repo() {
        // Skip silently when git is unavailable (minimal sandboxes).
        if !std::process::Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let url = init_test_repo(&tmp.path().join("src"), &["v0.1.0", "v0.2.0"]);

        assert_eq!(
            ls_remote_latest_tag(&url).unwrap().as_deref(),
            Some("v0.2.0")
        );

        // Shallow clone at an exact tag materialises the worktree.
        let dest = tmp.path().join("clone");
        clone_tag_shallow(&url, "v0.2.0", &dest).unwrap();
        assert!(dest.join("f.txt").exists());

        // An unreachable remote is an Err, not a panic.
        let bad = format!("file://{}", tmp.path().join("nope").display());
        assert!(ls_remote_latest_tag(&bad).is_err());
    }

    #[test]
    fn pat_parsing_from_git_urls() {
        let pat = "t".repeat(44); // synthetic 44-char token, not a real credential
        assert_eq!(
            parse_pat_from_git_url(&format!(
                "https://{pat}@github.com/Local-DE-Coach/HyprFetch.git"
            )),
            Some(pat.to_string())
        );
        assert_eq!(
            parse_pat_from_git_url(&format!("https://x-access-token:{pat}@github.com/o/r.git")),
            Some(pat.to_string())
        );
        assert_eq!(
            parse_pat_from_git_url(&format!("https://oauth2:{pat}@github.com/o/r")),
            Some(pat.to_string())
        );
        assert_eq!(
            // Any username works with a PAT on GitHub — the common
            // `git clone https://<user>:<PAT>@…` shape.
            parse_pat_from_git_url(&format!("https://someuser:{pat}@github.com/o/r.git")),
            Some(pat.to_string())
        );
        // Negative cases.
        assert_eq!(parse_pat_from_git_url("https://github.com/o/r.git"), None);
        assert_eq!(
            parse_pat_from_git_url("git@github.com:Local-DE-Coach/HyprFetch.git"),
            None
        );
        assert_eq!(
            parse_pat_from_git_url("https://short@github.com/o/r"),
            None,
            "tiny strings are not PATs"
        );
        assert_eq!(
            parse_pat_from_git_url("https://user:pass@gitlab.com/o/r"),
            None,
            "non-github hosts are ignored"
        );
    }
}
