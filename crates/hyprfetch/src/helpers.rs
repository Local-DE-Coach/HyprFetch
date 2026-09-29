//! Shared helpers: config file model, XDG paths, API-token resolution and
//! the in-app updater configuration (self-hosted update channel only).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use hyprfetch_core::update::{UpdateConfig, DEFAULT_CHANNEL_URL};

/// `update` section of the config file.
///
/// Legacy keys (`repo`, `token`, `source_dir`, `git_url`) from older
/// versions are accepted but IGNORED — the updater talks only to the
/// self-hosted channel and never touches GitHub.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct UpdateCfg {
    /// Self-hosted update-channel base URL (`latest.json` mirror). One
    /// fast HTTPS GET, no GitHub, works even when the repo is private.
    /// Set to `""` to disable the updater entirely.
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

/// Resolve the updater config: `--channel` flag > `HYPRFETCH_UPDATE_CHANNEL`
/// env > `[update] channel` in the config file > built-in default
/// (<https://istias.tech/hyprfetch/updates/>). An empty value (flag, env or
/// config) means the user explicitly DISABLED the updater.
pub fn resolve_update_cfg(
    channel_flag: Option<&str>,
    cfg_update: &Option<UpdateCfg>,
) -> UpdateConfig {
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

    UpdateConfig { channel_url }
}

#[cfg(test)]
mod tests {
    use super::*;

    // One sequential test: it mutates HYPRFETCH_UPDATE_CHANNEL, and cargo
    // runs tests in parallel threads that share the process environment.
    #[test]
    fn resolve_precedence_and_disable() {
        // Flag wins even when env + config are set.
        let cfg = UpdateCfg {
            channel: Some("https://config.example/".into()),
        };
        std::env::set_var("HYPRFETCH_UPDATE_CHANNEL", "https://env.example/");
        let resolved = resolve_update_cfg(Some("https://flag.example/"), &Some(cfg.clone()));
        assert_eq!(resolved.channel_url, "https://flag.example/");

        // No flag: env wins over config.
        let resolved = resolve_update_cfg(None, &Some(cfg.clone()));
        assert_eq!(resolved.channel_url, "https://env.example/");

        // Neither: config wins over the built-in default.
        std::env::remove_var("HYPRFETCH_UPDATE_CHANNEL");
        let resolved = resolve_update_cfg(None, &Some(cfg.clone()));
        assert_eq!(resolved.channel_url, "https://config.example/");

        // Nothing at all: the project mirror.
        let resolved = resolve_update_cfg(None, &None);
        assert_eq!(resolved.channel_url, DEFAULT_CHANNEL_URL);

        // Empty STRING = opted out (distinct from unset, which defaults).
        let cfg = UpdateCfg {
            channel: Some(String::new()),
        };
        let resolved = resolve_update_cfg(None, &Some(cfg));
        assert_eq!(resolved.channel_url, "");
        assert!(resolved.effective_channel().is_none());

        // Flag can also disable.
        let resolved = resolve_update_cfg(Some(""), &None);
        assert_eq!(resolved.channel_url, "");
        assert!(resolved.effective_channel().is_none());
    }

    #[test]
    fn legacy_config_keys_are_ignored() {
        // Old configs carried repo/token/git_url/source_dir — they must
        // still parse (serde ignores unknown fields) and not error out.
        let raw = r#"
[update]
repo = "Local-DE-Coach/HyprFetch"
token = "ghp_legacytokenvalue123456"
source_dir = "/tmp/clone"
git_url = "git@github.com:Local-DE-Coach/HyprFetch.git"
channel = "https://mirror.example/"
"#;
        let cfg: ConfigFile = toml::from_str(raw).expect("legacy keys must not break parsing");
        let u = cfg.update.expect("update section present");
        assert_eq!(u.channel.as_deref(), Some("https://mirror.example/"));
    }
}
