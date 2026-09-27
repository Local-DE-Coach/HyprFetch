//! Route handlers.
//!
//! All handlers take `State<AppState>` and produce `Result<Json<T>, ApiError>`.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use hyprfetch_db::schema::{QosOverride, TaskState};
use hyprfetch_db::{EventsRepo, SegmentRow, SegmentState, SettingsRepo, TaskRow, TasksRepo};

use crate::error::ApiError;
use crate::{AppState, TaskListFilter};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn new_task_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

fn db<'a>(state: &'a AppState) -> TasksRepo<'a> {
    TasksRepo::new(&state.db)
}

// ---------------------------------------------------------------------------
// GET /healthz
// ---------------------------------------------------------------------------

/// `GET /healthz` — returns `{"status":"ok"}`.
pub async fn healthz() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok"}))
}

// ---------------------------------------------------------------------------
// GET /api/tasks
// ---------------------------------------------------------------------------

/// Query params for `GET /api/tasks`.
#[derive(Debug, Deserialize, Default)]
pub struct ListTasksQuery {
    pub state: Option<String>,
}

/// Response shape for `GET /api/tasks`.
#[derive(Serialize)]
pub struct ListTasksResponse {
    pub tasks: Vec<TaskDto>,
}

/// Public DTO for a task. Does not include per-segment progress — that's
/// available via `GET /api/tasks/:id`.
#[derive(Serialize)]
pub struct TaskDto {
    pub id: String,
    pub url: String,
    pub filename: String,
    pub save_path: String,
    pub total_bytes: Option<i64>,
    pub downloaded_bytes: i64,
    pub state: TaskState,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub accept_ranges: bool,
    pub segments_requested: i64,
    pub qos_override: Option<QosOverride>,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

impl From<TaskRow> for TaskDto {
    fn from(r: TaskRow) -> Self {
        Self {
            id: r.id,
            url: r.url,
            filename: r.filename,
            save_path: r.save_path,
            total_bytes: r.total_bytes,
            downloaded_bytes: r.downloaded_bytes,
            state: r.state,
            etag: r.etag,
            last_modified: r.last_modified,
            accept_ranges: r.accept_ranges,
            segments_requested: r.segments_requested,
            qos_override: r.qos_override,
            error: r.error_message,
            created_at: r.created_at,
            updated_at: r.updated_at,
            completed_at: r.completed_at,
        }
    }
}

pub async fn list_tasks(
    State(state): State<AppState>,
    Query(q): Query<ListTasksQuery>,
) -> Result<Json<ListTasksResponse>, ApiError> {
    let filter = TaskListFilter::parse(q.state.as_deref());
    // For the filter we issue one query per desired state — keeps the code simple
    // and avoids string-interpolated IN clauses.
    let mut tasks: Vec<TaskRow> = Vec::new();
    for s in filter.states() {
        let mut more = db(&state).list_by_state(Some(*s))?;
        tasks.append(&mut more);
    }
    tasks.sort_by_key(|t| std::cmp::Reverse(t.created_at));
    let dtos: Vec<TaskDto> = tasks.into_iter().map(TaskDto::from).collect();
    Ok(Json(ListTasksResponse { tasks: dtos }))
}

// ---------------------------------------------------------------------------
// POST /api/tasks
// ---------------------------------------------------------------------------

/// Request body for `POST /api/tasks`.
#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub urls: Vec<String>,
    pub save_dir: Option<String>,
    pub filename: Option<String>,
    pub segments: Option<i64>,
    pub qos_override: Option<QosOverride>,
    pub headers: Option<HashMap<String, String>>,
    /// Currently informational — the engine treats all new tasks as
    /// auto-starting. Will be honored once the scheduler queue lands.
    #[allow(dead_code)]
    pub start_now: Option<bool>,
}

/// Response shape for `POST /api/tasks`.
#[derive(Serialize)]
pub struct CreateTaskResponse {
    pub tasks: Vec<TaskDto>,
}

pub async fn create_task(
    State(state): State<AppState>,
    Json(req): Json<CreateTaskRequest>,
) -> Result<(axum::http::StatusCode, Json<CreateTaskResponse>), ApiError> {
    if req.urls.is_empty() {
        return Err(ApiError::InvalidRequest("urls must not be empty".into()));
    }
    if req.urls.len() > 100 {
        return Err(ApiError::InvalidRequest(
            "too many urls (max 100 per request)".into(),
        ));
    }

    let save_dir = req.save_dir.unwrap_or_else(|| {
        SettingsRepo::new(&state.db)
            .get("download_dir")
            .ok()
            .flatten()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()) + "/Downloads"
            })
    });

    let segments = req.segments.unwrap_or_else(|| {
        SettingsRepo::new(&state.db)
            .get("segments_default")
            .ok()
            .flatten()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8)
    });
    if !(1..=32).contains(&segments) {
        return Err(ApiError::InvalidRequest(
            "segments must be in 1..=32".into(),
        ));
    }

    let extra_headers = req
        .headers
        .as_ref()
        .map(|h| serde_json::to_string(h).unwrap_or_default());

    let now = now_ms();
    let mut created: Vec<TaskRow> = Vec::with_capacity(req.urls.len());

    for url in req.urls.iter() {
        validate_url(url)?;

        let filename = req.filename.clone().unwrap_or_else(|| {
            url.rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "download.bin".into())
        });
        let save_path = format!("{save_dir}/{filename}");

        let row = TaskRow {
            id: new_task_id(),
            url: url.clone(),
            filename: filename.clone(),
            save_path,
            total_bytes: None,
            downloaded_bytes: 0,
            state: TaskState::Queued,
            etag: None,
            last_modified: None,
            accept_ranges: false,
            segments_requested: segments,
            qos_override: req.qos_override,
            extra_headers: extra_headers.clone(),
            error_message: None,
            created_at: now,
            updated_at: now,
            completed_at: None,
        };

        db(&state).insert(&row)?;
        EventsRepo::new(&state.db)
            .append(Some(&row.id), "task.created", "{}")
            .ok();
        created.push(row);
    }

    // Auto-start: kick the queue pump instead of starting each task
    // directly. The pump enforces the `max_concurrent_tasks` setting —
    // tasks beyond the cap stay `queued` and start automatically as slots
    // free up. Explicit user resumes bypass the cap (user intent).
    state.engine.pump().await;
    let dtos = created.into_iter().map(TaskDto::from).collect();
    Ok((
        axum::http::StatusCode::CREATED,
        Json(CreateTaskResponse { tasks: dtos }),
    ))
}

/// Basic URL validation. Rejects non-HTTP(S) schemes. SSRF check on host
/// is applied at fetch time (the segmented downloader), not here — but we
/// reject `file://` / `ftp://` etc. up front so the user gets immediate feedback.
fn validate_url(url: &str) -> Result<(), ApiError> {
    let parsed = url::Url::parse(url).map_err(|e| ApiError::InvalidUrl(e.to_string()))?;
    match parsed.scheme() {
        "http" | "https" => Ok(()),
        other => Err(ApiError::InvalidUrl(format!(
            "scheme must be http or https, got {other}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// GET /api/tasks/:id  +  DELETE /api/tasks/:id
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct TaskDetailResponse {
    pub task: TaskDto,
    pub segments: Vec<SegmentDto>,
}

#[derive(Serialize)]
pub struct SegmentDto {
    pub id: i64,
    pub start: i64,
    pub end: i64,
    pub current: i64,
    pub state: SegmentState,
    pub speed_bps: i64,
    pub error: Option<String>,
}

impl From<SegmentRow> for SegmentDto {
    fn from(r: SegmentRow) -> Self {
        Self {
            id: r.segment_idx,
            start: r.start_byte,
            end: r.end_byte,
            current: r.current_byte,
            state: r.state,
            speed_bps: r.speed_bps,
            error: r.error_message,
        }
    }
}

pub async fn get_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TaskDetailResponse>, ApiError> {
    let task = db(&state)
        .get(&id)?
        .ok_or_else(|| ApiError::TaskNotFound(id.clone()))?;
    let segments = hyprfetch_db::SegmentsRepo::new(&state.db)
        .list_for_task(&id)?
        .into_iter()
        .map(SegmentDto::from)
        .collect();
    Ok(Json(TaskDetailResponse {
        task: TaskDto::from(task),
        segments,
    }))
}

pub async fn delete_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<DeleteTaskQuery>,
) -> Result<axum::http::StatusCode, ApiError> {
    let row = db(&state)
        .get(&id)?
        .ok_or_else(|| ApiError::TaskNotFound(id.clone()))?;
    db(&state).delete(&id)?;
    EventsRepo::new(&state.db)
        .append(Some(&id), "task.removed", "{}")
        .ok();

    // `?delete_file=true` also removes the (partially) downloaded file from
    // disk. Best-effort: a missing file is not an error — the task row is
    // already gone either way.
    if q.delete_file == Some(true) && !row.save_path.is_empty() {
        match std::fs::remove_file(&row.save_path) {
            Ok(()) => tracing::info!(task = %id, path = %row.save_path, "deleted task file"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(task = %id, path = %row.save_path, error = %e, "could not delete task file")
            }
        }
    }
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Query params for `DELETE /api/tasks/:id`.
#[derive(Debug, Deserialize, Default)]
pub struct DeleteTaskQuery {
    /// When `true`, also delete the downloaded (partial) file from disk.
    pub delete_file: Option<bool>,
}

// ---------------------------------------------------------------------------
// POST /api/tasks/:id/{pause,resume,cancel}
// ---------------------------------------------------------------------------

async fn transition(
    state: &AppState,
    id: &str,
    target: TaskState,
    action_label: &str,
) -> Result<Json<TaskDto>, ApiError> {
    let row = db(state)
        .get(id)?
        .ok_or_else(|| ApiError::TaskNotFound(id.to_string()))?;

    let allowed = match target {
        TaskState::Paused => matches!(row.state, TaskState::Queued | TaskState::Downloading),
        TaskState::Downloading => matches!(row.state, TaskState::Queued | TaskState::Paused),
        // Retry: an errored task goes back to the queue (clears the error).
        TaskState::Queued => matches!(row.state, TaskState::Error),
        TaskState::Removed => matches!(
            row.state,
            TaskState::Queued
                | TaskState::Downloading
                | TaskState::Paused
                | TaskState::Complete
                | TaskState::Error
        ),
        _ => false,
    };
    if !allowed {
        return Err(ApiError::InvalidStateTransition(format!(
            "cannot {} a task in state {}",
            action_label, row.state
        )));
    }

    // Moving a task back to the queue (retry) clears its error message;
    // other transitions preserve it.
    let clear_error = target == TaskState::Queued;
    db(state).touch(
        id,
        target,
        row.downloaded_bytes,
        if clear_error {
            None
        } else {
            row.error_message.as_deref()
        },
    )?;
    EventsRepo::new(&state.db)
        .append(Some(id), &format!("task.{action_label}"), "{}")
        .ok();

    let updated = db(state)
        .get(id)?
        .ok_or_else(|| ApiError::InternalError("task disappeared after transition".into()))?;
    Ok(Json(TaskDto::from(updated)))
}

pub async fn pause_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TaskDto>, ApiError> {
    let updated = transition(&state, &id, TaskState::Paused, "paused").await?;
    // Also tell the engine to stop workers.
    let _ = state.engine.pause(&id).await; // ignore "not running" — task may be paused from queued state
    Ok(updated)
}

pub async fn resume_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TaskDto>, ApiError> {
    let updated = transition(&state, &id, TaskState::Downloading, "resumed").await?;
    // Kick the engine to start (or restart) the workers.
    if let Err(e) = state.engine.start(&id).await {
        tracing::warn!(task = %id, error = %e, "engine.start() failed on resume");
        // Non-fatal: the task is in Downloading state in the DB; user can retry.
    }
    Ok(updated)
}

pub async fn cancel_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TaskDto>, ApiError> {
    let updated = transition(&state, &id, TaskState::Removed, "cancelled").await?;
    let _ = state.engine.cancel(&id).await;
    Ok(updated)
}

// ---------------------------------------------------------------------------
// POST /api/tasks/:id/retry
// ---------------------------------------------------------------------------

/// `POST /api/tasks/:id/retry` — re-run an errored task.
///
/// Clears the error, moves the task back to `queued`, and kicks the queue
/// pump. Persisted segment offsets are kept: if the remote is unchanged the
/// retry resumes from the last written byte; if it changed, the coordinator's
/// validator recheck discards the offsets and restarts from byte 0.
pub async fn retry_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TaskDto>, ApiError> {
    let updated = transition(&state, &id, TaskState::Queued, "retried").await?;
    state.engine.pump().await;
    Ok(updated)
}

// ---------------------------------------------------------------------------
// GET/PUT /api/qos
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
pub struct QosState {
    pub enabled: bool,
    pub target_bps: Option<u64>,
}

pub async fn get_qos(State(state): State<AppState>) -> Result<Json<QosState>, ApiError> {
    let s = SettingsRepo::new(&state.db);
    let enabled = s.get("qos_enabled")?.as_deref() == Some("true");
    let target_bps: Option<u64> = s
        .get("qos_target_bps")?
        .and_then(|v| v.parse().ok())
        .filter(|v: &u64| *v > 0);
    Ok(Json(QosState {
        enabled,
        target_bps,
    }))
}

pub async fn set_qos(
    State(state): State<AppState>,
    Json(req): Json<QosState>,
) -> Result<Json<QosState>, ApiError> {
    let s = SettingsRepo::new(&state.db);
    s.set("qos_enabled", if req.enabled { "true" } else { "false" })?;
    s.set("qos_target_bps", &req.target_bps.unwrap_or(0).to_string())?;
    // Apply live to the engine-wide limiter so running tasks pick up the
    // new rate immediately (the limiter is shared by all active tasks).
    state
        .engine
        .set_qos(req.enabled, req.target_bps.unwrap_or(0));
    EventsRepo::new(&state.db)
        .append(
            None,
            "qos.set",
            &serde_json::to_string(&serde_json::json!({
                "enabled": req.enabled,
                "target_bps": req.target_bps,
            }))
            .unwrap_or_default(),
        )
        .ok();
    Ok(Json(req))
}

// ---------------------------------------------------------------------------
// GET/PATCH /api/settings
// ---------------------------------------------------------------------------

pub async fn get_settings(
    State(state): State<AppState>,
) -> Result<Json<HashMap<String, String>>, ApiError> {
    Ok(Json(
        SettingsRepo::new(&state.db).all()?.into_iter().collect(),
    ))
}

#[derive(Debug, Deserialize)]
pub struct PatchSettings {
    #[serde(flatten)]
    pub changes: HashMap<String, String>,
}

pub async fn patch_settings(
    State(state): State<AppState>,
    Json(req): Json<PatchSettings>,
) -> Result<Json<HashMap<String, String>>, ApiError> {
    if req.changes.is_empty() {
        return Err(ApiError::InvalidRequest("no settings to patch".into()));
    }
    if req.changes.len() > 50 {
        return Err(ApiError::InvalidRequest(
            "too many settings (max 50)".into(),
        ));
    }
    let s = SettingsRepo::new(&state.db);
    for (k, v) in &req.changes {
        s.set(k, v)?;
    }
    Ok(Json(s.all()?.into_iter().collect()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router;
    use axum::body::{to_bytes, Body};
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let db = hyprfetch_db::open_in_memory().expect("open_in_memory should succeed");
        crate::make_state(db)
    }

    async fn body_str(body: Body) -> String {
        let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn healthz_returns_ok() {
        let app = router(test_state());
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        assert!(body.contains("\"ok\""));
    }

    #[tokio::test]
    async fn list_tasks_empty_returns_empty_array() {
        let app = router(test_state());
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/tasks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        assert!(body.contains("\"tasks\":[]"));
    }

    #[tokio::test]
    async fn create_task_validates_url_scheme() {
        let app = router(test_state());
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({"urls":["ftp://example.com/x"]}))
                            .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let body = body_str(res.into_body()).await;
        assert!(body.contains("invalid_url"), "got: {body}");
    }

    #[tokio::test]
    async fn create_task_rejects_empty_urls() {
        let app = router(test_state());
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({"urls":[]})).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_then_get_then_delete_roundtrip() {
        // Use a shared AppState so all sub-requests hit the same in-memory DB.
        let state = test_state();
        let make_app = || router(state.clone());

        // POST a task
        let res = make_app()
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "urls": ["https://example.com/file.bin"],
                            "save_dir": "/tmp",
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = body_str(res.into_body()).await;
        let id = serde_json::from_str::<serde_json::Value>(&body).unwrap()["tasks"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // GET it back
        let res = make_app()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/tasks/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        assert!(body.contains("\"queued\""));
        assert!(body.contains("\"file.bin\""));

        // DELETE it
        let res = make_app()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/tasks/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);

        // GET again — 404
        let res = make_app()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/tasks/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn pause_then_resume_transition() {
        let app = router(test_state());
        // Create
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "urls": ["https://example.com/a.bin"],
                            "save_dir": "/tmp",
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_str(res.into_body()).await;
        let id = serde_json::from_str::<serde_json::Value>(&body).unwrap()["tasks"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Pause
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/tasks/{id}/pause"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        assert!(body.contains("\"paused\""), "got: {body}");

        // Resume
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/tasks/{id}/resume"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        assert!(body.contains("\"downloading\""), "got: {body}");
    }

    #[tokio::test]
    async fn pause_on_removed_returns_409() {
        // Cover the invalid-transition path: create → cancel → pause should 409.
        let app = router(test_state());
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "urls": ["https://example.com/c.bin"],
                            "save_dir": "/tmp",
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_str(res.into_body()).await;
        let id = serde_json::from_str::<serde_json::Value>(&body).unwrap()["tasks"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Cancel first — valid transition queued → removed.
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/tasks/{id}/cancel"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // Pause after cancel: invalid transition → 409.
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/tasks/{id}/pause"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        let body = body_str(res.into_body()).await;
        assert!(body.contains("invalid_state_transition"), "got: {body}");
    }

    #[tokio::test]
    async fn get_qos_default_is_disabled() {
        let app = router(test_state());
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/qos")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        assert!(body.contains("\"enabled\":false"));
    }

    #[tokio::test]
    async fn set_qos_applies_to_engine_limiter_live() {
        let state = test_state();
        let app = router(state.clone());
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/qos")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "enabled": true,
                            "target_bps": 250_000,
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // The engine-wide limiter must reflect the new config immediately —
        // running tasks share this bucket.
        assert!(state.engine.qos().is_enabled());
        assert_eq!(state.engine.qos().target_bps(), 250_000);

        // Turning it off must disable the limiter too.
        let res = router(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/qos")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "enabled": false,
                            "target_bps": 0,
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(!state.engine.qos().is_enabled());
    }

    #[tokio::test]
    async fn get_settings_returns_seeded_defaults() {
        let app = router(test_state());
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/settings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        assert!(body.contains("bind"));
        assert!(body.contains("127.0.0.1:7780"));
        assert!(body.contains("segments_default"));
    }

    // -- embedded SPA ---------------------------------------------------

    #[tokio::test]
    async fn ui_index_is_served() {
        let app = router(test_state());
        let res = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let ct = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert!(ct.starts_with("text/html"), "content-type: {ct}");
        let body = body_str(res.into_body()).await;
        assert!(body.contains("HyprFetch"), "index.html should name the app");
    }

    #[tokio::test]
    async fn ui_assets_are_served_and_api_404s_are_not_spa() {
        let app = router(test_state());

        // A real hashed asset must be reachable at its embedded path.
        let assets = crate::ui::asset_names();
        let asset = assets
            .iter()
            .find(|p| p.starts_with("assets/"))
            .expect("built SPA should contain hashed assets");

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/{asset}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "asset {asset} should exist");
        let cache = res
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(cache.contains("immutable"), "hashed assets cache forever");

        // Unknown API namespace must 404 as JSON/plain — NOT the SPA.
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/nope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn ui_deep_links_fall_back_to_spa() {
        let app = router(test_state());
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/downloads/history")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        assert!(body.contains("HyprFetch"));
    }

    // -- retry endpoint ---------------------------------------------------

    /// Seed a task directly in a given state (bypassing the create API).
    fn seed_raw_task(state: &AppState, st: TaskState, save_path: &str) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let id = uuid::Uuid::now_v7().to_string();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let row = hyprfetch_db::TaskRow {
            id: id.clone(),
            url: "https://example.com/r.bin".into(),
            filename: "r.bin".into(),
            save_path: save_path.into(),
            total_bytes: Some(10),
            downloaded_bytes: 4,
            state: st,
            etag: None,
            last_modified: None,
            accept_ranges: true,
            segments_requested: 2,
            qos_override: None,
            extra_headers: None,
            error_message: Some("only 0 of 2 segments completed".into()),
            created_at: now,
            updated_at: now,
            completed_at: None,
        };
        hyprfetch_db::TasksRepo::new(&state.db)
            .insert(&row)
            .unwrap();
        id
    }

    #[tokio::test]
    async fn retry_moves_errored_task_to_queued_and_clears_error() {
        let state = test_state();
        let id = seed_raw_task(&state, TaskState::Error, "/tmp/hyprfetch-retry-api.bin");
        let res = router(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/tasks/{id}/retry"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["state"], "queued", "retry must re-queue the task");
        assert!(
            v["error_message"].is_null(),
            "retry must clear the error message, got: {v}"
        );
        let _ = std::fs::remove_file("/tmp/hyprfetch-retry-api.bin");
    }

    #[tokio::test]
    async fn retry_on_queued_task_returns_409() {
        let state = test_state();
        let id = seed_raw_task(&state, TaskState::Queued, "/tmp/hyprfetch-retry-q.bin");
        let res = router(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/tasks/{id}/retry"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn retry_on_missing_task_returns_404() {
        let res = router(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/nope/retry")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    // -- delete with file removal -----------------------------------------

    #[tokio::test]
    async fn delete_with_delete_file_removes_file_from_disk() {
        let state = test_state();
        let path = "/tmp/hyprfetch-del-api.bin";
        std::fs::write(path, b"partial data").unwrap();
        let id = seed_raw_task(&state, TaskState::Paused, path);

        let res = router(state)
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/tasks/{id}?delete_file=true"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        assert!(
            !std::path::Path::new(path).exists(),
            "delete_file=true must remove the file from disk"
        );
    }

    #[tokio::test]
    async fn delete_without_param_keeps_file_on_disk() {
        let state = test_state();
        let path = "/tmp/hyprfetch-keep-api.bin";
        std::fs::write(path, b"partial data").unwrap();
        let id = seed_raw_task(&state, TaskState::Paused, path);

        let res = router(state)
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/tasks/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        assert!(
            std::path::Path::new(path).exists(),
            "plain delete must keep the file"
        );
        let _ = std::fs::remove_file(path);
    }
}
