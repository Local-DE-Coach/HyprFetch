//! HTTP + WebSocket server (axum). Serves the embedded SPA and the REST/WS API.
//!
//! See `docs/api.md` for the full API contract.

#![forbid(unsafe_code)]

mod auth;
mod error;
mod routes;
mod ui;
mod ws;

pub use auth::require_bearer;

pub use error::{ApiError, ApiErrorCode};

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use hyprfetch_db::schema::TaskState;

/// Shared application state passed to every handler.
#[derive(Clone)]
pub struct AppState {
    /// Database handle (Arc'd Mutex around a single rusqlite Connection).
    pub db: Arc<std::sync::Mutex<rusqlite::Connection>>,
    /// Download engine.
    pub engine: Arc<hyprfetch_core::Engine>,
}

/// Construct the shared AppState from a DB connection.
pub fn make_state(db: Arc<std::sync::Mutex<rusqlite::Connection>>) -> AppState {
    let engine = Arc::new(hyprfetch_core::Engine::new(Arc::clone(&db)));
    AppState { db, engine }
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
            "/api/tasks/:id",
            axum::routing::get(routes::get_task).delete(routes::delete_task),
        )
        .route(
            "/api/tasks/:id/pause",
            axum::routing::post(routes::pause_task),
        )
        .route(
            "/api/tasks/:id/resume",
            axum::routing::post(routes::resume_task),
        )
        .route(
            "/api/tasks/:id/cancel",
            axum::routing::post(routes::cancel_task),
        )
        .route(
            "/api/tasks/:id/retry",
            axum::routing::post(routes::retry_task),
        )
        .route(
            "/api/qos",
            axum::routing::get(routes::get_qos).put(routes::set_qos),
        )
        .route(
            "/api/settings",
            axum::routing::get(routes::get_settings).patch(routes::patch_settings),
        )
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
pub async fn serve_with_token(
    state: AppState,
    addr: SocketAddr,
    token: impl Into<Option<String>>,
) -> anyhow::Result<()> {
    let app = router_with_token(state, token);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "hyprfetch API listening");
    axum::serve(listener, app).await?;
    Ok(())
}
