//! HyprFetch binary entry point.
//!
//! Run modes (Node.js-style multi-run):
//! - `hyprfetch dev` — foreground, verbose pretty logs, opens the UI.
//! - `hyprfetch serve` — foreground prod server (compact logs).
//! - `hyprfetch daemon start|stop|restart|status` — detached prod server
//!   with a PID file + rotating log files, pm2-style lifecycle.
//! - `hyprfetch logs [-f]` — tail the daemon log.
//! - `hyprfetch update` — in-app self-update from GitHub releases.
//!
//! Configuration precedence (highest wins):
//! CLI flag > `HYPRFETCH_*` env var > config file > built-in default.

mod daemon;
mod helpers;
mod logger;
mod update_cmd;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use logger::LogMode;

#[derive(Debug, Parser)]
#[command(
    name = "hyprfetch",
    version,
    about = "Minimal-RAM download manager with a browser UI",
    after_help = "Run modes:\n  hyprfetch dev                 verbose dev console + auto-open UI\n  hyprfetch serve               prod server in the foreground\n  hyprfetch daemon start [--…]  run detached (logs via `hyprfetch logs -f`)\n  hyprfetch daemon stop|restart|status\n  hyprfetch logs [-f] [-n 100]  tail daemon logs\n  hyprfetch update [--check]    in-app self-update"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the download manager server (HTTP + WebSocket + UI).
    Serve {
        #[command(flatten)]
        opts: ServeFlags,
        /// Log verbosity preset: `dev` (verbose, pretty) or `prod` (quiet).
        #[arg(long, env = "HYPRFETCH_MODE", default_value = "prod")]
        mode: String,
        /// Open the web UI in the default browser once the server is up.
        #[arg(long, default_value_t = false)]
        open: bool,
    },
    /// Dev mode: verbose pretty logs, debug tracing, auto-opens the UI.
    Dev {
        #[command(flatten)]
        opts: ServeFlags,
    },
    /// Verify configuration and exit.
    Doctor,
    /// Manage the detached background server (pm2-style lifecycle).
    Daemon {
        #[command(subcommand)]
        cmd: DaemonCommand,
    },
    /// Show (and optionally follow) the daemon log file.
    Logs {
        /// Keep streaming new log lines (like `tail -f`).
        #[arg(short = 'f', long, default_value_t = false)]
        follow: bool,
        /// How many existing lines to print before following.
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: usize,
    },
    /// Check for / install new releases straight from GitHub.
    Update {
        /// Only report what's available; don't install.
        #[arg(long, default_value_t = false)]
        check: bool,
        /// Assume yes for the install prompt (script-friendly).
        #[arg(long, short = 'y', default_value_t = false)]
        yes: bool,
        /// Override the GitHub repo (`owner/name`).
        #[arg(long)]
        repo: Option<String>,
        /// GitHub token for private repos (else: env/config/settings).
        #[arg(long, env = "HYPRFETCH_GITHUB_TOKEN", hide_env_values = true)]
        token: Option<String>,
        /// Update by pulling + rebuilding a source clone instead of a release tarball.
        #[arg(long, default_value_t = false)]
        from_git: bool,
        /// Source clone used with --from-git.
        #[arg(long)]
        source_dir: Option<PathBuf>,
    },
}

/// Flags shared by `serve` and `dev`.
#[derive(Debug, clap::Args)]
struct ServeFlags {
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
    /// Tokio worker threads. Low default keeps idle RAM minimal —
    /// the workload is IO-bound. `0` = auto (one per core).
    #[arg(long, env = "HYPRFETCH_WORKERS", default_value_t = 2)]
    workers: usize,
    /// Exit the server after this many minutes with no active downloads and
    /// no open UI clients (a "sleep" mode — relaunch on demand). `0` = off.
    #[arg(long, env = "HYPRFETCH_EXIT_WHEN_IDLE", default_value_t = 0.0)]
    exit_when_idle: f64,
}

#[derive(Debug, Subcommand)]
enum DaemonCommand {
    /// Start a detached prod server. Extra args are passed to `serve`.
    Start {
        #[arg(allow_hyphen_values = true, trailing_var_arg = true)]
        serve_args: Vec<String>,
    },
    /// Stop the running daemon (SIGTERM, then SIGKILL fallback).
    Stop,
    /// Restart: stop (if running) then start again (same args by default).
    Restart {
        #[arg(allow_hyphen_values = true, trailing_var_arg = true)]
        serve_args: Vec<String>,
    },
    /// Show whether the daemon runs, plus live server info.
    Status,
}

/// Arguments for the `update` subcommand, extracted from the CLI enum.
pub struct UpdateArgs {
    pub check: bool,
    pub yes: bool,
    pub repo: Option<String>,
    pub token: Option<String>,
    pub from_git: bool,
    pub source_dir: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let orig_args: Vec<String> = std::env::args().skip(1).collect();

    // Fully-blocking lifecycle commands — no async runtime needed.
    match &cli.command {
        Command::Daemon {
            cmd: DaemonCommand::Start { serve_args },
        } => return daemon::start(serve_args),
        Command::Daemon {
            cmd: DaemonCommand::Stop,
        } => return daemon::stop(),
        Command::Daemon {
            cmd: DaemonCommand::Restart { serve_args },
        } => return daemon::restart(serve_args),
        Command::Daemon {
            cmd: DaemonCommand::Status,
        } => return daemon::status(),
        Command::Logs { follow, lines } => return daemon::logs(*follow, *lines),
        _ => {}
    }

    // Everything else runs inside a small tokio runtime. The worker count is
    // deliberately low: HyprFetch is IO-bound and a lean runtime keeps the
    // idle RAM/CPU footprint below typical download managers.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    rt.block_on(run_async(cli, orig_args))
}

async fn run_async(cli: Cli, orig_args: Vec<String>) -> Result<()> {
    match cli.command {
        Command::Serve { opts, mode, open } => {
            let mode = match mode.as_str() {
                "dev" => LogMode::Dev,
                "prod" => LogMode::Prod,
                other => bail!("invalid --mode {other} (expected dev|prod)"),
            };
            run_serve(opts, mode, open, orig_args).await
        }
        Command::Dev { opts } => run_serve(opts, LogMode::Dev, true, orig_args).await,
        Command::Doctor => doctor().await,
        Command::Update {
            check,
            yes,
            repo,
            token,
            from_git,
            source_dir,
        } => {
            update_cmd::run(UpdateArgs {
                check,
                yes,
                repo,
                token,
                from_git,
                source_dir,
            })
            .await
        }
        Command::Daemon { .. } | Command::Logs { .. } => unreachable!("handled in main()"),
    }
}

/// The full `serve` path, shared by `serve`, `dev` and the daemon child.
async fn run_serve(
    opts: ServeFlags,
    mode: LogMode,
    open: bool,
    orig_args: Vec<String>,
) -> Result<()> {
    logger::init(mode, None)?;

    let cfg = helpers::load_config(opts.config.as_deref())?;

    // Precedence: CLI/env > config file > default.
    let bind = opts
        .bind
        .or(cfg.bind)
        .unwrap_or_else(|| "127.0.0.1:7780".to_string());
    let db_path = opts
        .db_path
        .or(cfg.db_path)
        .unwrap_or_else(helpers::default_db_path);
    let download_dir = opts.download_dir.or(cfg.download_dir);
    let segments = opts.segments.or(cfg.segments);
    let allow_private = opts.allow_private || cfg.allow_private.unwrap_or(false);
    let config_token = opts.api_token.as_deref().or(cfg.api_token.as_deref());

    ensure_parent_dir(&db_path)?;
    tracing::info!(db = %db_path.display(), %bind, mode = ?mode, segments = ?segments, allow_private, "starting hyprfetch");

    let db = hyprfetch_db::open(&db_path)
        .with_context(|| format!("opening db at {}", db_path.display()))?;

    // Persist the download_dir override into settings if provided.
    {
        let s = hyprfetch_db::SettingsRepo::new(&db);
        if let Some(dir) = &download_dir {
            s.set(hyprfetch_core::SET_DOWNLOAD_DIR, &dir.to_string_lossy())
                .context("writing download_dir setting")?;
        }
        // Persist the segments override into settings if provided.
        if let Some(segs) = segments {
            s.set("segments_default", &segs.to_string())
                .context("writing segments_default setting")?;
        }

        // Category folders: created automatically so the user never has to
        // mkdir anything. Runs on every startup (idempotent) and whenever
        // directory settings change via the API.
        let all: std::collections::BTreeMap<String, String> = s
            .all()
            .context("reading settings for folder layout")?
            .into_iter()
            .collect();
        let base = all
            .get(hyprfetch_core::SET_DOWNLOAD_DIR)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                // Seed the default so the settings UI shows the real value.
                let def = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()) + "/Downloads";
                s.set(hyprfetch_core::SET_DOWNLOAD_DIR, &def).ok();
                def
            });
        let created = hyprfetch_core::categories::ensure_all_dirs(&base, &all);
        if !created.is_empty() {
            for d in &created {
                tracing::info!(dir = %d.display(), "created category folder");
            }
            tracing::info!(base = %base, "download folder layout ensured");
        }
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
    let (token, generated) = helpers::resolve_or_generate_token(config_token, &settings)?;
    if generated {
        tracing::info!(
            path = %helpers::token_file_path().display(),
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

    // In-app updater configuration (private-repo aware).
    let update_cfg = helpers::resolve_update_cfg_with_db(None, None, &cfg.update, Some(&settings));

    let state = hyprfetch_api::AppState {
        update_cfg: Arc::new(update_cfg),
        ..hyprfetch_api::AppState::with_defaults(db, engine.clone())
    };

    // Make the exact serve arguments available for in-app restarts.
    let _ = hyprfetch_api::SERVE_ARGS.set(orig_args);

    // Idle watcher ("sleep" mode): exit after N fully-idle minutes so the
    // app never sits resident doing nothing, unless explicitly disabled.
    if opts.exit_when_idle > 0.0 {
        spawn_idle_watcher(
            engine.clone(),
            state.ws_clients.clone(),
            state.shutdown.clone(),
            Duration::from_secs_f64(opts.exit_when_idle * 60.0),
        );
        tracing::info!(
            minutes = opts.exit_when_idle,
            "exit-when-idle enabled — the server will sleep after quiet time"
        );
    }

    if mode == LogMode::Dev {
        println!("┌─────────────────────────────────────────────┐");
        println!("│ HyprFetch dev mode — debug logs, live UI    │");
        println!("│ ui: http://{addr:<32}│");
        println!("└─────────────────────────────────────────────┘");
    }
    if open && loopback {
        let url = format!("http://{addr}");
        std::thread::spawn(move || {
            let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
        });
    }

    let api_token = if loopback { None } else { Some(token) };
    hyprfetch_api::serve_with_token(state, addr, api_token).await?;
    tracing::info!("server stopped cleanly");
    Ok(())
}

/// Spawn the low-frequency idle watchdog. Tick interval scales with the
/// configured idle budget (never faster than 1s, never slower than 30s).
fn spawn_idle_watcher(
    engine: Arc<hyprfetch_core::Engine>,
    ws_clients: Arc<std::sync::atomic::AtomicUsize>,
    shutdown: Arc<tokio::sync::Notify>,
    budget: Duration,
) {
    tokio::spawn(async move {
        let tick = (budget / 8)
            .max(Duration::from_secs(1))
            .min(Duration::from_secs(30));
        let mut idle_for = Duration::ZERO;
        loop {
            tokio::time::sleep(tick).await;
            let busy = engine.active_task_count().unwrap_or(1) > 0
                || ws_clients.load(std::sync::atomic::Ordering::Relaxed) > 0;
            if busy {
                idle_for = Duration::ZERO;
            } else {
                idle_for += tick;
                if idle_for >= budget {
                    tracing::info!(
                        secs = budget.as_secs(),
                        "fully idle for the configured budget — sleeping (exit)"
                    );
                    shutdown.notify_waiters();
                    return;
                }
            }
        }
    });
}

async fn doctor() -> Result<()> {
    logger::init(LogMode::Prod, None)?;
    let db_path = helpers::default_db_path();
    println!("hyprfetch doctor:");
    println!("  version      = {}", env!("CARGO_PKG_VERSION"));
    println!("  db path      = {}", db_path.display());
    println!("  default bind = 127.0.0.1:7780");
    println!("  state dir    = {}", daemon::state_dir().display());
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
    let token_file = helpers::token_file_path();
    println!(
        "  api token    = {}",
        if token_file.exists() {
            format!("configured ({})", token_file.display())
        } else {
            "not generated yet (created on first non-loopback serve)".to_string()
        }
    );
    let update_cfg = helpers::resolve_update_cfg_with_db(None, None, &None, Some(&s));
    println!("  update repo  = {}", update_cfg.repo);
    println!(
        "  update token = {}",
        if update_cfg.token.is_some() {
            "configured"
        } else {
            "not set (public repos only)"
        }
    );
    println!("  foreign_keys = ON (verified at open)");
    println!("  journal_mode = WAL (verified at open)");
    println!("OK");
    Ok(())
}

fn ensure_parent_dir(path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating dir {}", parent.display()))?;
    }
    Ok(())
}
