//! HyprFetch binary entry point.
//!
//! Wires CLI parsing, config-file loading, logging, and the engine + HTTP
//! server. Configuration precedence (highest wins):
//! CLI flag > `HYPRFETCH_*` env var > config file > built-in default.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde::Deserialize;

#[derive(Debug, Parser)]
#[command(
    name = "hyprfetch",
    version,
    about = "Minimal-RAM download manager with a browser UI"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the download manager server (HTTP + WebSocket + UI).
    Serve {
        #[arg(long, env = "HYPRFETCH_BIND")]
        bind: Option<String>,
        #[arg(long, env = "HYPRFETCH_DB")]
        db_path: Option<PathBuf>,
        #[arg(long, env = "HYPRFETCH_DOWNLOAD_DIR")]
        download_dir: Option<PathBuf>,
        #[arg(long, env = "HYPRFETCH_SEGMENTS")]
        segments: Option<u8>,
        /// Path to the config file. Defaults to
        /// `$XDG_CONFIG_HOME/hyprfetch/config.toml` (usually
        /// `~/.config/hyprfetch/config.toml`) when it exists.
        #[arg(long, env = "HYPRFETCH_CONFIG")]
        config: Option<PathBuf>,
        /// API token required for non-loopback binds. When absent, a token
        /// is resolved from the config/settings and finally generated and
        /// stored at `~/.config/hyprfetch/token`.
        #[arg(long, env = "HYPRFETCH_API_TOKEN", hide_env_values = true)]
        api_token: Option<String>,
        /// Allow downloads from loopback/private IP ranges. NOT recommended
        /// in production — disables SSRF protection. Useful for testing.
        #[arg(long, env = "HYPRFETCH_ALLOW_PRIVATE", default_value_t = false)]
        allow_private: bool,
    },
    /// Verify configuration and exit.
    Doctor,
}

/// Flat config-file model. Unknown keys are ignored so newer fields don't
/// break older binaries.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ConfigFile {
    bind: Option<String>,
    db_path: Option<PathBuf>,
    download_dir: Option<PathBuf>,
    segments: Option<u8>,
    api_token: Option<String>,
    allow_private: Option<bool>,
}

fn default_config_path() -> Option<PathBuf> {
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

fn load_config(explicit: Option<&Path>) -> Result<ConfigFile> {
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

fn default_db_path() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home).join(".local").join("share")
        });
    base.join("hyprfetch").join("hyprfetch.db")
}

fn token_file_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home).join(".config")
        });
    base.join("hyprfetch").join("token")
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating dir {}", parent.display()))?;
    }
    Ok(())
}

/// Resolve the API token: CLI/env > config file > settings DB > token file.
/// Generates + persists a fresh token when nothing is configured anywhere.
/// Returns `(token, was_generated)`.
fn resolve_or_generate_token(
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("hyprfetch=info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Serve {
            bind,
            db_path,
            download_dir,
            segments,
            config,
            api_token,
            allow_private,
        } => {
            let cfg = load_config(config.as_deref())?;

            // Precedence: CLI/env > config file > default.
            let bind = bind
                .or(cfg.bind)
                .unwrap_or_else(|| "127.0.0.1:7780".to_string());
            let db_path = db_path.or(cfg.db_path).unwrap_or_else(default_db_path);
            let download_dir = download_dir.or(cfg.download_dir);
            let segments = segments.or(cfg.segments);
            let allow_private = allow_private || cfg.allow_private.unwrap_or(false);
            let config_token = api_token.as_deref().or(cfg.api_token.as_deref());

            ensure_parent_dir(&db_path)?;
            tracing::info!(db = %db_path.display(), %bind, segments = ?segments, allow_private, "starting hyprfetch");

            let db = hyprfetch_db::open(&db_path)
                .with_context(|| format!("opening db at {}", db_path.display()))?;

            // Persist the download_dir override into settings if provided.
            if let Some(dir) = &download_dir {
                let s = hyprfetch_db::SettingsRepo::new(&db);
                s.set("download_dir", &dir.to_string_lossy())
                    .context("writing download_dir setting")?;
            }
            // Persist the segments override into settings if provided.
            if let Some(segs) = segments {
                let s = hyprfetch_db::SettingsRepo::new(&db);
                s.set("segments_default", &segs.to_string())
                    .context("writing segments_default setting")?;
            }

            let addr: SocketAddr = bind
                .parse()
                .with_context(|| format!("invalid --bind {bind}"))?;
            let loopback = addr.ip().is_loopback();

            // SSRF policy: CLI `--allow-private` disables protection; the
            // `ssrf_block_private` setting (default: true) controls the rest.
            let ssrf_setting = hyprfetch_db::SettingsRepo::new(&db)
                .get("ssrf_block_private")
                .ok()
                .flatten()
                .and_then(|v| v.parse::<bool>().ok())
                .unwrap_or(true);
            let policy = hyprfetch_core::SsrfPolicy {
                block_private: !allow_private && ssrf_setting,
            };
            let engine = Arc::new(hyprfetch_core::Engine::with_ssrf_policy(
                Arc::clone(&db),
                policy,
            ));

            // Startup resume pass: reload incomplete tasks, validate the
            // remote (ETag / Last-Modified / size), restart from offsets.
            match engine.resume_all().await {
                Ok(n) if n > 0 => tracing::info!(tasks = n, "resumed incomplete tasks"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "startup resume pass failed"),
            }

            // API token: auto-generated on first run and stored at
            // `~/.config/hyprfetch/token`. Enforced only on non-loopback
            // binds (per docs/api.md).
            let settings = hyprfetch_db::SettingsRepo::new(&db);
            let (token, generated) = resolve_or_generate_token(config_token, &settings)?;
            if generated {
                tracing::info!(
                    path = %token_file_path().display(),
                    "generated new API token (stored at the path above)"
                );
            }
            if loopback {
                tracing::info!("loopback bind: API token not required");
            } else {
                tracing::warn!(
                    "non-loopback bind {addr}: API token REQUIRED for /api and /ws \
                     (Authorization: Bearer <token>, or ?access_token= for browser WS)"
                );
            }

            let state = hyprfetch_api::AppState { db, engine };
            let api_token = if loopback { None } else { Some(token) };
            hyprfetch_api::serve_with_token(state, addr, api_token).await?;
            Ok(())
        }
        Command::Doctor => {
            let db_path = default_db_path();
            println!("hyprfetch doctor:");
            println!("  db path      = {}", db_path.display());
            println!("  default bind = 127.0.0.1:7780");
            ensure_parent_dir(&db_path)?;
            let db = hyprfetch_db::open(&db_path)?;
            let s = hyprfetch_db::SettingsRepo::new(&db);
            println!("  bind (db)    = {}", s.get("bind")?.unwrap_or_default());
            println!(
                "  segments (db)= {}",
                s.get("segments_default")?.unwrap_or_default()
            );
            println!(
                "  max_concurrent (db)= {}",
                s.get("max_concurrent_tasks")?.unwrap_or_default()
            );
            let token_file = token_file_path();
            println!(
                "  api token    = {}",
                if token_file.exists() {
                    format!("configured ({})", token_file.display())
                } else {
                    "not generated yet (created on first non-loopback serve)".to_string()
                }
            );
            println!("  foreign_keys = ON (verified at open)");
            println!("  journal_mode = WAL (verified at open)");
            println!("OK");
            Ok(())
        }
    }
}
