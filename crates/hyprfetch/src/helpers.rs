//! Shared helpers: config file model, XDG paths, token resolution and the
//! in-app updater configuration.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use hyprfetch_core::update::{UpdateConfig, DEFAULT_API_BASE, DEFAULT_CHANNEL_URL, DEFAULT_REPO};

/// `update` section of the config file.
#[derive(Debug, Default, Deserialize)]
pub struct UpdateCfg {
    /// GitHub repo (`owner/name`) checked by the updater.
    pub repo: Option<String>,
    /// PAT used when the repo is private (sent as a Bearer token).
    pub token: Option<String>,
    /// Source clone path for `hyprfetch update --from-git`.
    pub source_dir: Option<PathBuf>,
    /// Git remote used when the REST API cannot see the repo (private repo
    /// + SSH access). Falls back to a local clone's origin automatically.
    pub git_url: Option<String>,
    /// Self-hosted update-channel base URL (`latest.json` mirror). Checked
    /// FIRST — one fast HTTPS GET, no GitHub, works for private repos.
    /// Set to `""` to disable the channel tier.
    pub channel: Option<String>,
}

/// Flat config-file model. Unknown keys are ignored so newer fields don't
/// break older binaries.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ConfigFile {
    pub bind: Option<String>,
    pub db_path: Option<PathBuf>,
    pub download_dir: Option<PathBuf>,
    pub segments: Option<u8>,
    pub api_token: Option<String>,
    pub allow_private: Option<bool>,
    /// Tokio worker threads (default 2 — the workload is IO-bound).
    pub workers: Option<usize>,
    /// Sleep mode: exit after N idle minutes (0 = off).
    pub exit_when_idle: Option<f64>,
    /// In-app updater settings.
    pub update: Option<UpdateCfg>,
}

pub fn default_config_path() -> Option<PathBuf> {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".config"))
                .ok()
        })?;
    Some(base.join("hyprfetch").join("config.toml"))
}

pub fn load_config(explicit: Option<&Path>) -> Result<ConfigFile> {
    let path = match explicit {
        Some(p) => Some(p.to_path_buf()),
        None => default_config_path().filter(|p| p.exists()),
    };
    let Some(path) = path else {
        return Ok(ConfigFile::default());
    };
    if !path.exists() {
        bail!("config file not found: {}", path.display());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading config file {}", path.display()))?;
    let cfg: ConfigFile =
        toml::from_str(&raw).with_context(|| format!("parsing config file {}", path.display()))?;
    tracing::info!(path = %path.display(), "loaded config file");
    Ok(cfg)
}

pub fn default_db_path() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home).join(".local").join("share")
        });
    base.join("hyprfetch").join("hyprfetch.db")
}

pub fn token_file_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home).join(".config")
        });
    base.join("hyprfetch").join("token")
}

pub fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating dir {}", parent.display()))?;
    }
    Ok(())
}

/// Resolve the API token: CLI/env > config file > settings DB > token file.
/// Generates + persists a fresh token when nothing is configured anywhere.
/// Returns `(token, was_generated)`.
pub fn resolve_or_generate_token(
    cli_token: Option<&str>,
    settings: &hyprfetch_db::SettingsRepo,
) -> Result<(String, bool)> {
    if let Some(t) = cli_token.map(str::trim).filter(|t| !t.is_empty()) {
        return Ok((t.to_string(), false));
    }
    if let Some(t) = settings
        .get("api_token")?
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return Ok((t, false));
    }
    if let Ok(existing) = std::fs::read_to_string(token_file_path()) {
        let t = existing.trim();
        if !t.is_empty() {
            return Ok((t.to_string(), false));
        }
    }

    // Nothing configured: generate a fresh token and persist it.
    let token = format!("hpf_{}", uuid::Uuid::now_v7().simple());
    let path = token_file_path();
    ensure_parent_dir(&path)?;
    std::fs::write(&path, format!("{token}\n"))
        .with_context(|| format!("writing token file {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    settings.set("api_token", &token)?;
    Ok((token, true))
}

/// Common clone locations scanned when the updater has no explicit token.
/// `git clone https://<PAT>@github.com/…` (the documented install path) puts
/// the PAT right into the origin URL, so a plain clone install needs zero
/// extra configuration for `hyprfetch update` to work.
const CLONE_SCAN_PATHS: &[&str] = &[
    "HyprFetch",
    "Projects/HyprFetch",
    "src/HyprFetch",
    "code/HyprFetch",
    "Developer/HyprFetch",
];

/// Directories scanned when the updater has no explicit token/remote:
/// `[update] source_dir` first, then well-known clone locations under $HOME.
fn clone_candidate_dirs(source_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(d) = source_dir {
        candidates.push(d.to_path_buf());
    }
    if let Ok(home) = std::env::var("HOME") {
        for rel in CLONE_SCAN_PATHS {
            candidates.push(Path::new(&home).join(rel));
        }
    }
    candidates
}

/// Scan well-known clone locations for git access to the repo. Returns
/// `(pat_from_origin_url, origin_url)`:
/// - the PAT covers HTTPS-PAT clones (zero-config API-tier updates),
/// - the origin URL covers SSH remotes — the updater's git tier can
///   `ls-remote`/`clone` with the user's existing SSH keys, so private-repo
///   updates work with NO token at all.
pub fn detect_git_from_source_clones(
    source_dir: Option<&Path>,
) -> (Option<String>, Option<String>) {
    for dir in clone_candidate_dirs(source_dir) {
        if !dir.join(".git").exists() {
            continue;
        }
        let url = std::process::Command::new("git")
            .args(["-C"])
            .arg(&dir)
            .arg("config")
            .arg("--get")
            .arg("remote.origin.url")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        let Some(url) = url.filter(|u| !u.is_empty()) else {
            continue;
        };
        let token = hyprfetch_core::update::parse_pat_from_git_url(&url);
        if token.is_some() || url.contains("github.com") {
            tracing::debug!(clone = %dir.display(), "found usable git access in a local clone");
            let url_out = url.contains("github.com").then(|| url.clone());
            return (token, url_out);
        }
    }
    (None, None)
}

/// PAT from the GitHub CLI (`gh auth login` users).
fn gh_cli_token() -> Option<String> {
    let out = std::process::Command::new("gh")
        .args(["auth", "token"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (t.len() >= 20).then_some(t)
}

/// PAT stored in git's credential helpers for github.com (store, cache,
/// libsecret, gnome-keyring…). Prompting is disabled: helpers that would
/// need user interaction simply fail and we move on.
fn git_credential_token() -> Option<String> {
    use std::io::Write as _;
    let mut child = std::process::Command::new("git")
        .args(["credential", "fill"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "true")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    {
        let mut si = child.stdin.take()?;
        let _ = si.write_all(b"protocol=https\nhost=github.com\n\n");
    }
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let pass = text
        .lines()
        .find_map(|l| l.strip_prefix("password="))
        .map(str::trim)
        .unwrap_or_default();
    let t = pass.to_string();
    (t.len() >= 20).then_some(t)
}

/// Resolve the updater config from CLI flags, env vars, the config file's
/// `[update]` section and the settings DB (in that precedence order).
#[allow(clippy::too_many_arguments)]
pub fn resolve_update_cfg_with_db(
    channel_flag: Option<&str>,
    repo_flag: Option<&str>,
    token_flag: Option<&str>,
    cfg_update: &Option<UpdateCfg>,
    settings: Option<&hyprfetch_db::SettingsRepo>,
) -> UpdateConfig {
    let db_repo = settings
        .and_then(|s| s.get("github_repo").ok().flatten())
        .filter(|v| !v.trim().is_empty());
    let db_token = settings
        .and_then(|s| s.get("github_token").ok().flatten())
        .filter(|v| !v.trim().is_empty());

    let repo = repo_flag
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            cfg_update
                .as_ref()
                .and_then(|u| u.repo.clone())
                .filter(|s| !s.trim().is_empty())
        })
        .or(db_repo)
        .unwrap_or_else(|| DEFAULT_REPO.to_string());

    // Ordered credential discovery — first hit wins and is labelled so the
    // CLI can show WHERE it came from. The clone scan doubles as the git
    // tier's remote discovery (SSH clones grant tag access without a token).
    let (clone_token, clone_url) =
        detect_git_from_source_clones(cfg_update.as_ref().and_then(|u| u.source_dir.as_deref()));

    let mut token: Option<String> = None;
    let mut token_source: Option<&'static str> = None;
    let mut take = |cand: Option<String>, label: &'static str| {
        if token.is_none() {
            if let Some(t) = cand.filter(|t| !t.trim().is_empty()) {
                token = Some(t.trim().to_string());
                token_source = Some(label);
            }
        }
    };
    // clap may have filled the flag from HYPRFETCH_GITHUB_TOKEN already.
    take(
        token_flag
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        "cli flag/env",
    );
    take(std::env::var("HYPRFETCH_GITHUB_TOKEN").ok(), "env");
    take(std::env::var("GITHUB_TOKEN").ok(), "env");
    take(std::env::var("GH_TOKEN").ok(), "env");
    take(
        cfg_update.as_ref().and_then(|u| u.token.clone()),
        "config file",
    );
    take(db_token, "settings db");
    take(clone_token, "clone origin");
    take(gh_cli_token(), "gh cli");
    take(git_credential_token(), "git credentials");

    let git_url = cfg_update
        .as_ref()
        .and_then(|u| u.git_url.clone())
        .filter(|s| !s.trim().is_empty())
        .or(clone_url);

    let api_base = std::env::var("HYPRFETCH_UPDATE_API")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_API_BASE.to_string());

    // Channel precedence: --channel flag > HYPRFETCH_UPDATE_CHANNEL env >
    // [update] channel in the config file > built-in default. Empty value
    // disables the tier ("" means the user explicitly opted out).
    let channel_url = [
        channel_flag,
        std::env::var("HYPRFETCH_UPDATE_CHANNEL").ok().as_deref(),
        cfg_update.as_ref().and_then(|u| u.channel.as_deref()),
    ]
    .into_iter()
    .flatten()
    .find(|s| !s.trim().is_empty() || s.is_empty())
    .map(str::to_string)
    .unwrap_or_else(|| DEFAULT_CHANNEL_URL.to_string());

    UpdateConfig {
        repo,
        token,
        token_source,
        git_url,
        api_base,
        channel_url,
    }
}
