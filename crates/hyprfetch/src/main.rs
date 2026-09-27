//! HyprFetch binary entry point.
//!
//! Wires CLI parsing, logging, config loading, and the engine + HTTP server.

use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

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
        #[arg(long, default_value = "127.0.0.1:7780", env = "HYPRFETCH_BIND")]
        bind: String,
        #[arg(long, env = "HYPRFETCH_DB")]
        db_path: Option<PathBuf>,
        #[arg(long, env = "HYPRFETCH_DOWNLOAD_DIR")]
        download_dir: Option<PathBuf>,
        #[arg(long, default_value_t = 8, env = "HYPRFETCH_SEGMENTS")]
        segments: u8,
    },
    /// Verify configuration and exit.
    Doctor,
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

fn ensure_data_dir(db_path: &Path) -> Result<()> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating data dir {}", parent.display()))?;
    }
    Ok(())
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
        } => {
            let db_path = db_path.unwrap_or_else(default_db_path);
            ensure_data_dir(&db_path)?;
            tracing::info!(db = %db_path.display(), %bind, segments, "starting hyprfetch");

            let db = hyprfetch_db::open(&db_path)
                .with_context(|| format!("opening db at {}", db_path.display()))?;

            // Persist the download_dir override into settings if provided.
            if let Some(dir) = download_dir {
                let s = hyprfetch_db::SettingsRepo::new(&db);
                s.set("download_dir", &dir.to_string_lossy())
                    .context("writing download_dir setting")?;
            }

            let addr: SocketAddr = bind
                .parse()
                .with_context(|| format!("invalid --bind {bind}"))?;

            let state = hyprfetch_api::AppState {
                db: Arc::clone(&db),
            };
            hyprfetch_api::serve(state, addr).await?;
            Ok(())
        }
        Command::Doctor => {
            let db_path = default_db_path();
            println!("hyprfetch doctor:");
            println!("  db path      = {}", db_path.display());
            println!("  default bind = 127.0.0.1:7780");
            ensure_data_dir(&db_path)?;
            let db = hyprfetch_db::open(&db_path)?;
            let s = hyprfetch_db::SettingsRepo::new(&db);
            println!("  bind (db)    = {}", s.get("bind")?.unwrap_or_default());
            println!(
                "  segments (db)= {}",
                s.get("segments_default")?.unwrap_or_default()
            );
            println!("  foreign_keys = ON (verified at open)");
            println!("  journal_mode = WAL (verified at open)");
            println!("OK");
            Ok(())
        }
    }
}
