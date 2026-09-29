//! Shared helpers: config file model, XDG paths, token resolution and the
//! in-app updater configuration.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use hyprfetch_core::update::{UpdateConfig, DEFAULT_API_BASE, DEFAULT_REPO};

/// `update` section of the config file.
#[derive(Debug, Default, Deserialize)]
pub struct UpdateCfg {
    /// GitHub repo (`owner/name`) checked by the updater.
    pub repo: Option<String>,
    /// PAT used when the repo is private (sent as a Bearer token).
    pub token: Option<String>,
    /// Source clone path for `hyprfetch update --from-git`.
    pub source_dir: Option<PathBuf>,
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

/// Try to find a GitHub PAT in the origin URL of a local HyprFetch clone.
/// Checks the configured `[update] source_dir` first, then the well-known
/// clone locations under `$HOME`. Returns `None` when nothing is found.
pub fn detect_token_from_source_clones(source_dir: Option<&Path>) -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(d) = source_dir {
        candidates.push(d.to_path_buf());
    }
    for rel in CLONE_SCAN_PATHS {
        candidates.push(Path::new(&home).join(rel));
    }
    for dir in candidates {
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
        if let Some(token) = url
            .as_deref()
            .and_then(hyprfetch_core::update::parse_pat_from_git_url)
        {
            tracing::debug!(clone = %dir.display(), "resolved updater token from clone origin URL");
            return Some(token);
        }
    }
    None
}

/// Resolve the updater config from CLI flags, env vars, the config file's
/// `[update]` section and the settings DB (in that precedence order).
pub fn resolve_update_cfg(repo_flag: Option<&str>, token_flag: Option<&str>) -> UpdateConfig {
    let (cfg_update, settings) = (None, None);
    resolve_update_cfg_with_db(repo_flag, token_flag, &cfg_update, settings.as_ref())
}

/// Same, with the config-file `update` section and an open settings repo.
pub fn resolve_update_cfg_with_db(
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

    let token = token_flag
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var("HYPRFETCH_GITHUB_TOKEN")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(|| {
            std::env::var("GITHUB_TOKEN")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(|| {
            cfg_update
                .as_ref()
                .and_then(|u| u.token.clone())
                .filter(|s| !s.trim().is_empty())
        })
        .or(db_token)
        // Last resort for clone-based installs: the PAT lives in the origin
        // URL of the local source clone, so reuse it (zero-config updates).
        .or_else(|| {
            detect_token_from_source_clones(
                cfg_update.as_ref().and_then(|u| u.source_dir.as_deref()),
            )
        });

    let api_base = std::env::var("HYPRFETCH_UPDATE_API")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_API_BASE.to_string());

    UpdateConfig {
        repo,
        token,
        api_base,
    }
}
