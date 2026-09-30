//! HTTP + WebSocket server (axum). Serves the embedded SPA and the REST/WS API.
//!
//! See `docs/api.md` for the full API contract.

#![forbid(unsafe_code)]

mod auth;
mod error;
mod routes;
mod ui;
mod usage;
mod widget;
mod ws;

pub use auth::require_bearer;

pub use error::{ApiError, ApiErrorCode};
pub use usage::{Usage, UsageTracker};

use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use hyprfetch_core::update::{UpdateCheck, UpdateConfig};
use hyprfetch_db::schema::TaskState;
use tokio::sync::Notify;

/// Original CLI arguments of the running `hyprfetch serve` process.
/// Used by `POST /api/update/restart` to re-exec a replacement server.
pub static SERVE_ARGS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

/// A downloaded-but-not-yet-swapped update binary kept for the WebUI's
/// one-click update setup (`POST /api/update/authorize`). Filled when
/// `POST /api/update/apply` answers `needs_password`.
#[derive(Debug, Clone)]
pub struct StagedUpdate {
    /// Staged new binary (temp file, removed by the helper on success).
    pub staged: String,
    /// System path to replace (e.g. /usr/bin/hyprfetch).
    pub target: String,
    /// Version being installed.
    pub latest: String,
    /// Epoch seconds when this staging entry was created (expiry guard).
    pub created_at: u64,
}

/// Live status of the one-click update setup (terminal window running the
/// privileged one-time setup script).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AuthorizeState {
    /// A setup run is/was active.
    pub running: bool,
    /// Terminal emulator the setup window was opened in, if spawned.
    pub terminal: Option<String>,
    /// Epoch seconds when the terminal was spawned.
    pub started_at: Option<u64>,
    /// The staged binary was consumed — swap finished.
    pub done: bool,
    /// The daemon restart was triggered after the swap.
    pub restarted: bool,
    /// Human-readable failure (timeout, spawn failure, …).
    pub error: Option<String>,
}

/// Shared application state passed to every handler.
#[derive(Clone)]
pub struct AppState {
    /// Database handle (Arc'd Mutex around a single rusqlite Connection).
    pub db: Arc<std::sync::Mutex<rusqlite::Connection>>,
    /// Download engine.
    pub engine: Arc<hyprfetch_core::Engine>,
    /// Live WebSocket client count (for `/api/server` + the idle watcher).
    pub ws_clients: Arc<AtomicUsize>,
    /// Where the in-app updater checks for new releases.
    pub update_cfg: Arc<UpdateConfig>,
    /// Cached result of the last `GET /api/update/check`.
    pub update_cache: Arc<tokio::sync::Mutex<Option<UpdateCheck>>>,
    /// Pending staged update for the one-click setup (needs_password flow).
    pub staged_update: Arc<tokio::sync::Mutex<Option<StagedUpdate>>>,
    /// Status of the one-click update setup run.
    pub authorize_state: Arc<tokio::sync::Mutex<AuthorizeState>>,
    /// Notified once when the process should exit gracefully (restart / idle).
    pub shutdown: Arc<Notify>,
    /// When this server process came up (for `/api/server` uptime).
    pub started_at: Instant,
    /// Resource-usage sampler for `/api/system/usage` (this app only).
    pub usage: Arc<UsageTracker>,
}

impl AppState {
    /// Build state with defaults for everything except db + engine.
    /// Used by tests and by `make_state`.
    pub fn with_defaults(
        db: Arc<std::sync::Mutex<rusqlite::Connection>>,
        engine: Arc<hyprfetch_core::Engine>,
    ) -> AppState {
        AppState {
            db,
            engine,
            ws_clients: Arc::new(AtomicUsize::new(0)),
            update_cfg: Arc::new(UpdateConfig::default()),
            update_cache: Arc::new(tokio::sync::Mutex::new(None)),
            staged_update: Arc::new(tokio::sync::Mutex::new(None)),
            authorize_state: Arc::new(tokio::sync::Mutex::new(AuthorizeState::default())),
            shutdown: Arc::new(Notify::new()),
            started_at: Instant::now(),
            usage: Arc::new(UsageTracker::default()),
        }
    }
}

/// Construct the shared AppState from a DB connection.
pub fn make_state(db: Arc<std::sync::Mutex<rusqlite::Connection>>) -> AppState {
    let engine = Arc::new(hyprfetch_core::Engine::new(Arc::clone(&db)));
    AppState::with_defaults(db, engine)
}

/// Filter for the `?state=` query param on `GET /api/tasks`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskListFilter {
    /// `?state=active` — return queued + downloading + paused
    Active,
    /// `?state=completed` — return complete + error
    Completed,
    /// `?state=all` or omitted — return everything except removed
    All,
}

impl TaskListFilter {
    /// Parse from the raw query value. `None` maps to `All`.
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::to_ascii_lowercase).as_deref() {
            Some("active") => Self::Active,
            Some("completed") => Self::Completed,
            Some("all") | None => Self::All,
            Some(_) => Self::All, // unknown → all
        }
    }

    /// Return the SQL `state IN (...)` clause (without parentheses) and
    /// matching parameter list.
    pub fn states(self) -> &'static [TaskState] {
        match self {
            Self::Active => &[TaskState::Queued, TaskState::Downloading, TaskState::Paused],
            Self::Completed => &[TaskState::Complete, TaskState::Error],
            Self::All => &[
                TaskState::Queued,
                TaskState::Downloading,
                TaskState::Paused,
                TaskState::Complete,
                TaskState::Error,
            ],
        }
    }
}

/// Build the public axum router.
pub fn router(state: AppState) -> Router {
    router_with_token(state, None)
}

/// Build the router with Bearer-token auth enforced on `/api/*` and `/ws`.
///
/// Used when the daemon binds a non-loopback address. `/healthz` and the
/// embedded SPA stay open; clients present the token via
/// `Authorization: Bearer <token>` or `?access_token=` (browser WS).
pub fn router_with_token(state: AppState, token: impl Into<Option<String>>) -> Router {
    let token: Option<Arc<String>> = token.into().filter(|t| !t.is_empty()).map(Arc::new);
    let mut app = Router::new()
        .route("/ws", axum::routing::get(ws::ws_handler))
        .route(
            "/api/tasks",
            axum::routing::get(routes::list_tasks).post(routes::create_task),
        )
        .route(
            "/api/tasks/{id}",
            axum::routing::get(routes::get_task).delete(routes::delete_task),
        )
        .route(
            "/api/tasks/{id}/pause",
            axum::routing::post(routes::pause_task),
        )
        .route(
            "/api/tasks/{id}/resume",
            axum::routing::post(routes::resume_task),
        )
        .route(
            "/api/tasks/{id}/cancel",
            axum::routing::post(routes::cancel_task),
        )
        .route(
            "/api/tasks/{id}/retry",
            axum::routing::post(routes::retry_task),
        )
        .route(
            "/api/tasks/{id}/open",
            axum::routing::post(routes::open_task_file),
        )
        .route(
            "/api/tasks/{id}/reveal",
            axum::routing::post(routes::reveal_task),
        )
        .route("/api/inspect", axum::routing::post(routes::inspect_url))
        .route(
            "/api/qos",
            axum::routing::get(routes::get_qos).put(routes::set_qos),
        )
        .route(
            "/api/settings",
            axum::routing::get(routes::get_settings).patch(routes::patch_settings),
        )
        .route(
            "/api/categories",
            axum::routing::get(routes::get_categories),
        )
        .route("/api/server", axum::routing::get(routes::server_info))
        .route(
            "/api/update/check",
            axum::routing::get(routes::update_check),
        )
        .route(
            "/api/update/apply",
            axum::routing::post(routes::update_apply),
        )
        .route(
            "/api/update/authorize",
            axum::routing::post(routes::update_authorize),
        )
        .route(
            "/api/update/authorize/status",
            axum::routing::get(routes::update_authorize_status),
        )
        .route(
            "/api/update/restart",
            axum::routing::post(routes::update_restart),
        )
        .route(
            "/api/update/stale-copies/fix",
            axum::routing::post(routes::update_fix_stale_copies),
        )
        .route(
            "/api/widget/status",
            axum::routing::get(widget::widget_status),
        )
        .route(
            "/api/widget/install",
            axum::routing::post(widget::widget_install),
        )
        .route(
            "/api/widget/uninstall",
            axum::routing::post(widget::widget_uninstall),
        )
        .route("/api/open-folder", axum::routing::post(routes::open_folder))
        .route(
            "/api/system/usage",
            axum::routing::get(routes::system_usage),
        )
        .route("/api/power/quiet", axum::routing::post(routes::power_quiet))
        .route("/api/power/wake", axum::routing::post(routes::power_wake))
        .with_state(state.clone());

    if let Some(token) = token {
        app = app.layer(axum::middleware::from_fn(move |req, next| {
            let token = Arc::clone(&token);
            async move { auth::require_bearer(req, next, token).await }
        }));
    }

    // /healthz, the SPA, and the fallback are OUTSIDE the auth layer —
    // liveness probes and the login page must work without a token.
    app.route("/healthz", axum::routing::get(routes::healthz))
        .route("/", axum::routing::get(ui::index))
        .fallback(ui::static_path)
        .with_state(state)
}

/// Convenience: build router + bind + serve. Used by the binary.
pub async fn serve(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    serve_with_token(state, addr, None).await
}

/// Bind + serve with an optional API token (non-loopback binds MUST pass a
/// token here — `hyprfetch serve` resolves/generates it before calling).
///
/// Returns when `state.shutdown` is notified (restart / idle-exit) or on a
/// fatal accept error.
pub async fn serve_with_token(
    state: AppState,
    addr: SocketAddr,
    token: impl Into<Option<String>>,
) -> anyhow::Result<()> {
    let app = router_with_token(state.clone(), token);
    let listener = bind_with_retry(addr).await?;
    tracing::info!(%addr, "hyprfetch API listening");
    let shutdown = state.shutdown;
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown.notified().await;
            tracing::info!("shutdown signal received — draining connections");
        })
        .await?;
    Ok(())
}

/// Bind with a short retry window. This makes restart hand-offs race-free:
/// `POST /api/update/restart` spawns the replacement BEFORE the old process
/// releases the port, so the child may see `Address already in use` for a
/// moment. Retrying up to 15s keeps the hand-off deterministic.
async fn bind_with_retry(addr: SocketAddr) -> anyhow::Result<tokio::net::TcpListener> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => return Ok(l),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(e.into());
                }
                tracing::debug!(%addr, "port still held by previous instance — retrying bind");
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
}
