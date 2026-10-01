//! Route handlers.
//!
//! All handlers take `State<AppState>` and produce `Result<Json<T>, ApiError>`.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use hyprfetch_core::categories::{
    category_for_filename, dir_for_category, ensure_all_dirs, expand_tilde, is_valid_category,
    override_key, sanitize_filename, sniff_filename, CATEGORIES, SET_CATEGORIZE, SET_DOWNLOAD_DIR,
};
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

/// Percent-decode a raw URL path segment (`my%20file.zip` → `my file.zip`).
fn decode_segment(seg: &str) -> String {
    percent_encoding::percent_decode_str(seg)
        .decode_utf8_lossy()
        .into_owned()
}

/// Derive a download filename from a URL: the last path segment,
/// percent-decoded and sanitized (`…/images?q=tbn:ANd9…` → `images`).
/// Query strings and fragments never leak into the name. Never empty.
fn url_filename(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(parsed) => parsed
            .path_segments()
            .and_then(|mut segs| segs.next_back())
            .map(|s| sanitize_filename(&decode_segment(s)))
            .unwrap_or_else(|| "download.bin".into()),
        Err(_) => sanitize_filename(url.rsplit('/').next().unwrap_or("")),
    }
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
    /// `?source=extension` — only tasks initiated by the given source
    /// (`app` / `extension` / `media`). The Tasks page filter chips use it.
    pub source: Option<String>,
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
    /// Category derived from the filename extension (video / pictures /
    /// music / compress / documents / apps / other). Matches the folder the
    /// download is sorted into when auto-categorization is on.
    pub category: String,
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
    /// Where this task came from: `app` / `extension` / `media`.
    pub source: String,
    /// Parsed media metadata for media-engine tasks (quality, container…).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<serde_json::Value>,
}

impl From<TaskRow> for TaskDto {
    fn from(r: TaskRow) -> Self {
        Self {
            category: category_for_filename(&r.filename).to_string(),
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
            source: r.source,
            media: r.media_meta.and_then(|m| serde_json::from_str(&m).ok()),
        }
    }
}

pub async fn list_tasks(
    State(state): State<AppState>,
    Query(q): Query<ListTasksQuery>,
) -> Result<Json<ListTasksResponse>, ApiError> {
    let filter = TaskListFilter::parse(q.state.as_deref());
    let source = q
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "all");
    // For the filter we issue one query per desired state — keeps the code simple
    // and avoids string-interpolated IN clauses. The source dimension composes
    // with the state dimension via the repo's list_filtered.
    let mut tasks: Vec<TaskRow> = Vec::new();
    match source {
        Some(src) => {
            for s in filter.states() {
                let mut more = db(&state).list_filtered(Some(*s), Some(src))?;
                tasks.append(&mut more);
            }
        }
        None => {
            for s in filter.states() {
                let mut more = db(&state).list_by_state(Some(*s))?;
                tasks.append(&mut more);
            }
        }
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
    /// Direct-save directory: used verbatim (tilde-expanded) for every task
    /// in this request. When set, auto-categorization is skipped for these
    /// tasks — the file lands exactly where you point it.
    pub save_dir: Option<String>,
    /// `"auto"` (default) sorts by filename extension into the category
    /// folders under the base download dir; an explicit category name
    /// (`video`, `pictures`, …) forces that folder; `none` saves straight
    /// into the base dir.
    pub category: Option<String>,
    pub filename: Option<String>,
    pub segments: Option<i64>,
    pub qos_override: Option<QosOverride>,
    pub headers: Option<HashMap<String, String>>,
    /// Currently informational — the engine treats all new tasks as
    /// auto-starting. Will be honored once the scheduler queue lands.
    #[allow(dead_code)]
    pub start_now: Option<bool>,
}

/// Resolve the save directory for a new task from the request + settings.
///
/// Precedence: explicit `save_dir` (direct save) → explicit `category` →
/// auto-detect by extension (when `categorize` is on) → base download dir.
/// The chosen directory is created if missing.
pub(crate) fn resolve_save_dir(
    save_dir: Option<&str>,
    category: Option<&str>,
    filename: &str,
    settings: &std::collections::BTreeMap<String, String>,
) -> Result<String, ApiError> {
    // 1. Direct save: the caller picks the exact directory.
    if let Some(dir) = save_dir.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(expand_tilde(dir).to_string_lossy().into_owned());
    }

    // Base dir: setting > $HOME/Downloads (the Linux-desktop default).
    let base = settings
        .get(SET_DOWNLOAD_DIR)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()) + "/Downloads");

    // 2. Explicit category from the request. `"auto"` (the WebUI modal's
    //    default) means exactly the same as omitting the field: extension
    //    auto-detection in step 3. (Before 0.4.4 this rejected `"auto"`
    //    with 400 — the modal's recommended option could never start a
    //    download; caught by the v0.4.4 API battery.)
    match category.map(str::trim).filter(|s| !s.is_empty()) {
        Some("none") | Some("base") => {
            return Ok(expand_tilde(&base).to_string_lossy().into_owned());
        }
        Some(c) if is_valid_category(c) => {
            return Ok(dir_for_category(c, &base, settings)
                .to_string_lossy()
                .into_owned());
        }
        Some("auto") => { /* fall through to extension auto-detect */ }
        Some(other) => {
            return Err(ApiError::InvalidRequest(format!(
                "unknown category `{other}` (valid: {} or \"auto\"/\"none\")",
                CATEGORIES.join(", ")
            )));
        }
        None => {}
    }

    // 3. Auto-categorize by filename extension (default: on).
    let categorize = settings
        .get(SET_CATEGORIZE)
        .map(|s| s.trim() != "false")
        .unwrap_or(true);
    if categorize {
        let cat = category_for_filename(filename);
        return Ok(dir_for_category(cat, &base, settings)
            .to_string_lossy()
            .into_owned());
    }

    // 4. Plain base directory.
    Ok(expand_tilde(&base).to_string_lossy().into_owned())
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

    let settings_map: std::collections::BTreeMap<String, String> =
        SettingsRepo::new(&state.db).all()?.into_iter().collect();

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

        // Filename: explicit request value > last URL path segment (query
        // string stripped, percent-decoded). The engine re-sniffs the
        // extension from the server's Content-Type once the download starts
        // (v0.4.6), so extension-less URLs still land correctly.
        let filename = req
            .filename
            .clone()
            .filter(|s| !s.trim().is_empty())
            .map(|s| sanitize_filename(&s))
            .unwrap_or_else(|| url_filename(url));

        // Directory resolution: direct save > explicit category >
        // auto-detect by extension > base dir. Missing dirs are created so
        // the folder exists the moment the task is queued.
        let save_dir = resolve_save_dir(
            req.save_dir.as_deref(),
            req.category.as_deref(),
            &filename,
            &settings_map,
        )?;
        if let Err(e) = std::fs::create_dir_all(&save_dir) {
            tracing::warn!(dir = %save_dir, error = %e, "could not pre-create save dir");
        }
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
            source: "app".into(),
            media_meta: None,
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

/// Public wrapper for sibling modules (`media.rs`, `extension.rs`).
pub(crate) fn validate_public_url(url: &str) -> Result<(), ApiError> {
    validate_url(url)
}

/// Create a task with `source = "extension"` (browser-extension bridge).
/// Same filename/dir defaults as `create_task`; no `segments` knob (the
/// engine default of 8 is right for every media type the extension sees).
pub(crate) async fn create_extension_task(
    state: &AppState,
    url: &str,
    filename: Option<&str>,
    page_url: Option<&str>,
) -> Result<TaskDto, ApiError> {
    let extra_headers = page_url
        .filter(|p| !p.is_empty())
        .map(|p| serde_json::json!({ "page_url": p }).to_string());
    create_sourced_task(state, url, filename, "extension", extra_headers, 8).await
}

/// Create a task with `source = "media"` (yt-dlp quality-picked download).
pub(crate) async fn create_media_task(
    state: &AppState,
    url: &str,
    filename: &str,
    save_path: &str,
    media_meta: String,
) -> Result<TaskDto, ApiError> {
    create_sourced_task_full(
        state,
        url,
        filename,
        save_path,
        "media",
        None,
        Some(media_meta),
        1,
    )
    .await
}

/// Shared engine for `create_extension_task` / `create_media_task`:
/// resolve dir + filename, insert the row, kick the pump, return the DTO.
async fn create_sourced_task(
    state: &AppState,
    url: &str,
    filename: Option<&str>,
    source: &str,
    extra_headers: Option<String>,
    segments: i64,
) -> Result<TaskDto, ApiError> {
    let derived = filename
        .map(str::to_string)
        .unwrap_or_else(|| url_filename(url));
    let settings_map: std::collections::BTreeMap<String, String> =
        SettingsRepo::new(&state.db).all()?.into_iter().collect();
    let final_name = sanitize_filename(&derived);
    let save_dir = resolve_save_dir(None, None, &final_name, &settings_map)?;
    if let Err(e) = std::fs::create_dir_all(&save_dir) {
        tracing::warn!(dir = %save_dir, error = %e, "could not pre-create save dir");
    }
    let save_path = format!("{save_dir}/{final_name}");
    create_sourced_task_full(
        state,
        url,
        &final_name,
        &save_path,
        source,
        extra_headers,
        None,
        segments,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn create_sourced_task_full(
    state: &AppState,
    url: &str,
    filename: &str,
    save_path: &str,
    source: &str,
    extra_headers: Option<String>,
    media_meta: Option<String>,
    segments: i64,
) -> Result<TaskDto, ApiError> {
    validate_url(url)?;
    let now = now_ms();
    let row = TaskRow {
        id: new_task_id(),
        url: url.to_string(),
        filename: filename.to_string(),
        save_path: save_path.to_string(),
        total_bytes: None,
        downloaded_bytes: 0,
        state: TaskState::Queued,
        etag: None,
        last_modified: None,
        accept_ranges: false,
        segments_requested: segments,
        qos_override: None,
        extra_headers,
        error_message: None,
        created_at: now,
        updated_at: now,
        completed_at: None,
        source: source.to_string(),
        media_meta,
    };
    db(state).insert(&row)?;
    EventsRepo::new(&state.db)
        .append(
            Some(&row.id),
            "task.created",
            &format!("{{\"source\":\"{source}\"}}"),
        )
        .ok();
    state.engine.pump().await;
    Ok(TaskDto::from(row))
}

// ---------------------------------------------------------------------------
// POST /api/inspect
// ---------------------------------------------------------------------------

/// Request body for `POST /api/inspect`.
#[derive(Debug, Deserialize)]
pub struct InspectRequest {
    pub url: String,
    /// Same semantics as `POST /api/tasks` — used to resolve the exact
    /// destination path the confirm dialog shows.
    pub save_dir: Option<String>,
    pub category: Option<String>,
    pub filename: Option<String>,
}

/// Response for `POST /api/inspect` — everything the IDM-style confirm
/// dialog needs BEFORE a download starts: final URL (redirects resolved),
/// file name, size, range support and the resolved on-disk save path.
#[derive(Serialize)]
pub struct InspectResponse {
    pub url: String,
    /// URL after following redirects — the one the engine will actually fetch.
    pub final_url: String,
    pub filename: String,
    pub total_bytes: Option<i64>,
    pub accept_ranges: bool,
    pub category: String,
    pub save_dir: String,
    pub save_path: String,
    /// Server-reported media type — what the filename extension was
    /// sniffed from when the URL carried none.
    pub content_type: Option<String>,
}

/// `POST /api/inspect` — probe a URL (HEAD with SSRF + redirect checks) and
/// resolve where the file would land. Does NOT create a task, does NOT
/// create directories; purely informational for the confirm dialog.
pub async fn inspect_url(
    State(state): State<AppState>,
    Json(req): Json<InspectRequest>,
) -> Result<Json<InspectResponse>, ApiError> {
    validate_url(&req.url)?;
    let parsed = url::Url::parse(&req.url).map_err(|e| ApiError::InvalidUrl(e.to_string()))?;

    let probe = state
        .engine
        .inspect_url(&parsed)
        .await
        .map_err(|e| match e {
            hyprfetch_core::HttpError::Ssrf(_) => ApiError::SsrfBlocked(e.to_string()),
            other => ApiError::InvalidRequest(format!("probe failed: {other}")),
        })?;

    let response = inspect_response_from_probe_inner(
        &state,
        &req.url,
        probe,
        req.filename.as_deref(),
        req.save_dir.as_deref(),
        req.category.as_deref(),
    )?;
    Ok(Json(response))
}

/// Build an [`InspectResponse`] from a finished native probe + request
/// options. Shared by `POST /api/inspect` and the unified media probe
/// (`POST /api/media/probe`, `kind = "file"`).
pub(crate) fn inspect_response_from_probe(
    state: &AppState,
    url: &str,
    probe: hyprfetch_core::ProbeResult,
) -> Result<InspectResponse, ApiError> {
    inspect_response_from_probe_inner(state, url, probe, None, None, None)
}

fn inspect_response_from_probe_inner(
    state: &AppState,
    url: &str,
    probe: hyprfetch_core::ProbeResult,
    filename_override: Option<&str>,
    save_dir_opt: Option<&str>,
    category_opt: Option<&str>,
) -> Result<InspectResponse, ApiError> {
    // File name: explicit request value > last segment of the FINAL url
    // (redirects resolved) > generic fallback — mirrors create_task. The
    // extension is corrected from the probe's Content-Type (v0.4.6), so the
    // confirm dialog shows `images.jpg`, not `images?q=tbn:ANd9…`.
    let filename = filename_override
        .filter(|s| !s.trim().is_empty())
        .map(|s| sniff_filename(s, probe.content_type.as_deref()))
        .unwrap_or_else(|| {
            let raw = probe
                .final_url
                .path_segments()
                .and_then(|mut segs| segs.next_back())
                .unwrap_or("");
            sniff_filename(&decode_segment(raw), probe.content_type.as_deref())
        });

    let settings_map: std::collections::BTreeMap<String, String> =
        SettingsRepo::new(&state.db).all()?.into_iter().collect();
    let save_dir = resolve_save_dir(save_dir_opt, category_opt, &filename, &settings_map)?;
    let category = category_for_filename(&filename).to_string();

    let save_path = format!("{save_dir}/{filename}");
    Ok(InspectResponse {
        url: url.to_string(),
        final_url: probe.final_url.to_string(),
        filename,
        total_bytes: probe.content_length,
        accept_ranges: probe.accept_ranges,
        category,
        save_dir,
        save_path,
        content_type: probe.content_type,
    })
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
    // Kick the engine to start (or restart) the workers. Pausing is
    // asynchronous — right after a pause the old coordinator is still winding
    // down, so the first start attempt can transiently fail ("already
    // running"). Retry with a short backoff; if it still fails, roll the row
    // back to `paused` and return 409 so the task is never left stranded in
    // `downloading` with no workers attached (the engine also accepts a
    // stranded `downloading` row as a restartable state — this rollback is
    // the belt to that braces).
    let mut started = false;
    let mut last_err = None;
    for _ in 0..20 {
        match state.engine.start(&id).await {
            Ok(()) => {
                started = true;
                break;
            }
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
    if !started {
        let e = last_err.expect("retry loop runs at least once");
        tracing::warn!(task = %id, error = %e, "engine.start() failed on resume; rolling back to paused");
        db(&state).touch(&id, TaskState::Paused, updated.0.downloaded_bytes, None)?;
        return Err(ApiError::InvalidStateTransition(format!(
            "engine could not resume the task ({e}); task rolled back to paused"
        )));
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
// POST /api/tasks/:id/reveal  +  POST /api/tasks/:id/open
// ---------------------------------------------------------------------------

/// `POST /api/tasks/:id/reveal` — open the file's folder in the user's file
/// manager (the "GO" button in the WebUI). The file itself is NOT opened.
/// Works for any task whose save folder exists on disk.
pub async fn reveal_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let task = db(&state)
        .get(&id)?
        .ok_or_else(|| ApiError::TaskNotFound(id.clone()))?;
    let path = std::path::PathBuf::from(&task.save_path);
    let dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| path.clone());
    spawn_folder_opener(&dir, "folder").await
}

/// `POST /api/tasks/:id/open` — open the downloaded file with the system's
/// default application (the "Open" button in the WebUI; `xdg-open` on Linux).
/// Only finished downloads qualify — you never "open" a partial file.
pub async fn open_task_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let task = db(&state)
        .get(&id)?
        .ok_or_else(|| ApiError::TaskNotFound(id.clone()))?;
    if task.state != TaskState::Complete {
        return Err(ApiError::InvalidStateTransition(format!(
            "download not finished yet (state: {})",
            task.state
        )));
    }
    spawn_opener(std::path::Path::new(&task.save_path), "file")
}

/// Validate + hand a path to the desktop opener (`xdg-open`, or the program
/// named by `HYPRFETCH_FILE_OPENER` — used by tests and unusual setups).
///
/// Guard: the path must exist on disk, so a stale entry can never launch
/// anything. The opener is spawned detached and reaped in the background —
/// the HTTP call never blocks on a GUI app.
fn spawn_opener(path: &std::path::Path, what: &str) -> Result<Json<serde_json::Value>, ApiError> {
    if !path.exists() {
        return Err(ApiError::InvalidRequest(format!(
            "{what} not found on disk: {}",
            path.display()
        )));
    }

    let program = std::env::var("HYPRFETCH_FILE_OPENER")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "xdg-open".to_string());

    // Spawn detached; reap asynchronously so we never leak zombies.
    let mut cmd = tokio::process::Command::new(&program);
    cmd.arg(path);
    #[cfg(unix)]
    cmd.process_group(0);
    let child = cmd.spawn().map_err(|e| {
        ApiError::InternalError(format!(
            "could not launch `{program}` ({e}) — is a desktop environment running?"
        ))
    })?;
    tokio::spawn(async move {
        let mut child = child;
        let _ = child.wait().await;
    });

    tracing::info!(what, path = %path.display(), %program, "opened via desktop opener");
    Ok(Json(serde_json::json!({
        "opened": true,
        "what": what,
        "path": path.display().to_string(),
        "opener": program,
    })))
}

// ---------------------------------------------------------------------------
// Folder opening (v0.4.6): real file managers, never a terminal
// ---------------------------------------------------------------------------

/// GUI file managers tried in order when the desktop default for folders is
/// missing or points at a terminal. Ordered by Linux-desktop prevalence.
const FILE_MANAGERS: &[&str] = &[
    "nautilus",   // GNOME
    "dolphin",    // KDE
    "nemo",       // Cinnamon
    "thunar",     // Xfce (common on lightweight Hyprland setups too)
    "caja",       // MATE
    "pcmanfm-qt", // LXQt
    "pcmanfm",    // LXDE
    "krusader",   // KDE power users
    "spacefm",    // lightweight
    "doublecmd",  // dual-pane
];

/// Terminal emulators whose `.desktop` sometimes ends up as the
/// `inode/directory` handler (especially on minimal window-manager setups
/// like Hyprland + foot/kitty). `xdg-open <dir>` on such systems opens a
/// TERMINAL instead of a file manager — the exact v0.4.6 "GO sends me to
/// the terminal" bug report.
const TERMINAL_HINTS: &[&str] = &[
    "foot",
    "kitty",
    "alacritty",
    "wezterm",
    "ghostty",
    "st",
    "stterm",
    "urxvt",
    "rxvt",
    "xterm",
    "konsole",
    "gnome-terminal",
    "xfce4-terminal",
    "terminator",
    "tilix",
    "kittyterm",
    "qterminal",
    "lxterminal",
    "sakura",
    "termite",
    "contour",
    "wterm",
];

/// Look a program name up on PATH (like `command -v`).
fn find_on_path(program: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            use std::os::unix::fs::PermissionsExt;
            // Only usable when executable.
            if candidate
                .metadata()
                .ok()
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
            {
                return Some(candidate);
            }
        }
    }
    None
}

/// Does the resolved `inode/directory` handler look like a terminal
/// emulator? `<name>.desktop` file names are checked against known
/// terminal binaries (org.gnome.Terminal.desktop → "gnome-terminal" hits).
async fn default_dir_handler_is_terminal() -> bool {
    let probe = tokio::process::Command::new("xdg-mime")
        .args(["query", "default", "inode/directory"])
        .output();
    let output = match tokio::time::timeout(std::time::Duration::from_millis(1500), probe).await {
        Ok(Ok(o)) => o,
        // xdg-mime unavailable/broken/slow → don't trust xdg-open.
        _ => return true,
    };
    let desktop = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase();
    if desktop.is_empty() {
        return true; // no default at all → xdg-open would fall through to terminals
    }
    TERMINAL_HINTS
        .iter()
        .any(|h| desktop.contains(h) || desktop == format!("{h}.desktop"))
}

/// Pick the program that should open a FOLDER.
///
/// Order: `HYPRFETCH_FILE_OPENER` (tests / explicit override) → the desktop
/// default via `xdg-open`, UNLESS that default is missing or a terminal →
/// the first installed GUI file manager → `xdg-open` as the last resort.
async fn resolve_folder_opener(dir: &std::path::Path) -> String {
    if let Ok(p) = std::env::var("HYPRFETCH_FILE_OPENER") {
        if !p.trim().is_empty() {
            return p;
        }
    }

    if !default_dir_handler_is_terminal().await {
        return "xdg-open".to_string();
    }

    for fm in FILE_MANAGERS {
        if let Some(path) = find_on_path(fm) {
            tracing::debug!(
                dir = %dir.display(),
                fm = %fm,
                "desktop default for folders is missing/terminal — using file manager"
            );
            return path.display().to_string();
        }
    }
    // Nothing else to try — best effort.
    "xdg-open".to_string()
}

/// Open a directory with a REAL file manager (never a terminal).
/// See [`resolve_folder_opener`] for the selection order.
async fn spawn_folder_opener(
    dir: &std::path::Path,
    what: &str,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !dir.is_dir() {
        return Err(ApiError::InvalidRequest(format!(
            "{what} not found on disk: {}",
            dir.display()
        )));
    }

    let program = resolve_folder_opener(dir).await;

    let mut cmd = tokio::process::Command::new(&program);
    cmd.arg(dir);
    #[cfg(unix)]
    cmd.process_group(0);
    let child = cmd.spawn().map_err(|e| {
        ApiError::InternalError(format!(
            "could not launch `{program}` ({e}) — is a desktop environment running?"
        ))
    })?;
    tokio::spawn(async move {
        let mut child = child;
        let _ = child.wait().await;
    });

    tracing::info!(what, path = %dir.display(), %program, "opened folder");
    Ok(Json(serde_json::json!({
        "opened": true,
        "what": what,
        "path": dir.display().to_string(),
        "opener": program,
    })))
}

/// `POST /api/open-folder` — open one of HyprFetch's save folders in the
/// system file manager. Powers the clickable folder cards on the Dashboard.
/// Only paths at or under the configured save folders are allowed — a
/// compromised UI can never launch an arbitrary directory.
#[derive(Debug, Deserialize)]
pub struct OpenFolderRequest {
    pub path: String,
}

pub async fn open_folder(
    State(state): State<AppState>,
    Json(req): Json<OpenFolderRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let requested = expand_tilde(req.path.trim());
    let requested = requested
        .canonicalize()
        .map_err(|e| ApiError::InvalidRequest(format!("folder not found: {e}")))?;
    if !requested.is_dir() {
        return Err(ApiError::InvalidRequest(format!(
            "not a folder: {}",
            requested.display()
        )));
    }

    // Allowlist: base download dir + every category dir (+ their children).
    let settings: std::collections::BTreeMap<String, String> =
        SettingsRepo::new(&state.db).all()?.into_iter().collect();
    let base = base_download_dir(&settings);
    let mut allowed: Vec<std::path::PathBuf> =
        vec![expand_tilde(&base).to_string_lossy().into_owned().into()];
    for c in CATEGORIES {
        allowed.push(dir_for_category(c, &base, &settings));
    }
    let ok = allowed.iter().any(|root| {
        let root = root.canonicalize().unwrap_or_else(|_| root.clone());
        requested.starts_with(&root)
    });
    if !ok {
        return Err(ApiError::InvalidRequest(
            "folder is not a HyprFetch save folder".into(),
        ));
    }

    spawn_folder_opener(&requested, "folder").await
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
// GET /api/categories
// ---------------------------------------------------------------------------

/// One row of `GET /api/categories`.
#[derive(Serialize)]
pub struct CategoryInfo {
    pub name: String,
    /// Effective (tilde-expanded) directory for this category.
    pub dir: String,
    /// True when the user overrode the default `<base>/<name>` location.
    pub overridden: bool,
}

/// `GET /api/categories` — the category folder layout used to sort
/// downloads. Folders are created automatically (startup + settings change).
pub async fn get_categories(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let settings: std::collections::BTreeMap<String, String> =
        SettingsRepo::new(&state.db).all()?.into_iter().collect();
    let base = base_download_dir(&settings);
    let categorize = settings
        .get(SET_CATEGORIZE)
        .map(|s| s.trim() != "false")
        .unwrap_or(true);
    let cats: Vec<CategoryInfo> = CATEGORIES
        .iter()
        .map(|&c| {
            let overridden = settings
                .get(&override_key(c))
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            CategoryInfo {
                name: c.to_string(),
                dir: dir_for_category(c, &base, &settings)
                    .to_string_lossy()
                    .into_owned(),
                overridden,
            }
        })
        .collect();
    Ok(Json(serde_json::json!({
        "base": expand_tilde(&base).to_string_lossy(),
        "categorize": categorize,
        "categories": cats,
    })))
}

/// Base download dir from settings, with the `~/Downloads` default.
fn base_download_dir(settings: &std::collections::BTreeMap<String, String>) -> String {
    settings
        .get(SET_DOWNLOAD_DIR)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()) + "/Downloads")
}

// ---------------------------------------------------------------------------
// GET/PATCH /api/settings
// ---------------------------------------------------------------------------

/// Settings values that must never be echoed back to clients verbatim.
const SENSITIVE_SETTINGS: &[&str] = &["github_token", "api_token"];

/// Return all settings with secret values masked out (replaced by a
/// `<key>_set` boolean so the UI can show "configured" without the value).
fn masked_settings(
    repo: &SettingsRepo<'_>,
) -> Result<std::collections::BTreeMap<String, String>, ApiError> {
    let all = repo.all()?;
    let mut out = std::collections::BTreeMap::new();
    for (k, v) in all {
        if SENSITIVE_SETTINGS.contains(&k.as_str()) {
            out.insert(format!("{k}_set"), (!v.trim().is_empty()).to_string());
        } else {
            out.insert(k, v);
        }
    }
    Ok(out)
}

pub async fn get_settings(
    State(state): State<AppState>,
) -> Result<Json<std::collections::BTreeMap<String, String>>, ApiError> {
    Ok(Json(masked_settings(&SettingsRepo::new(&state.db))?))
}

#[derive(Debug, Deserialize)]
pub struct PatchSettings {
    #[serde(flatten)]
    pub changes: HashMap<String, String>,
}

/// Settings keys with a fixed value domain. Validated on PATCH so a typo or
/// a hostile client can't store garbage that the UI then chokes on.
/// Empty string clears the key (the UI falls back to its local default).
fn validate_setting(k: &str, v: &str) -> Result<(), ApiError> {
    let ok = match k {
        // Must mirror THEME_STYLES in the WebUI (ui/src/lib/theme.js).
        "ui_theme_style" => {
            v.is_empty() || matches!(v, "slate" | "ocean" | "forest" | "coffee" | "cyber")
        }
        "ui_theme_mode" => v.is_empty() || matches!(v, "dark" | "light"),
        "show_resource_usage" | "keep_alive_in_background" => {
            matches!(v, "true" | "false")
        }
        _ => true,
    };
    if ok {
        Ok(())
    } else {
        Err(ApiError::InvalidRequest(format!(
            "invalid value `{v}` for setting `{k}`"
        )))
    }
}

pub async fn patch_settings(
    State(state): State<AppState>,
    Json(req): Json<PatchSettings>,
) -> Result<Json<std::collections::BTreeMap<String, String>>, ApiError> {
    if req.changes.is_empty() {
        return Err(ApiError::InvalidRequest("no settings to patch".into()));
    }
    if req.changes.len() > 50 {
        return Err(ApiError::InvalidRequest(
            "too many settings (max 50)".into(),
        ));
    }
    for (k, v) in &req.changes {
        validate_setting(k, v)?;
    }
    let s = SettingsRepo::new(&state.db);
    for (k, v) in &req.changes {
        s.set(k, v)?;
    }

    // Directory-relevant changes rebuild the folder layout immediately so
    // the new location exists before the next download starts.
    let touched_dirs = req
        .changes
        .keys()
        .any(|k| k == SET_DOWNLOAD_DIR || k == SET_CATEGORIZE || k.starts_with("category_dir_"));
    if touched_dirs {
        let all: std::collections::BTreeMap<String, String> = s.all()?.into_iter().collect();
        let base = base_download_dir(&all);
        let created = ensure_all_dirs(&base, &all);
        for d in created {
            tracing::info!(dir = %d.display(), "created category folder");
        }
    }

    Ok(Json(masked_settings(&s)?))
}

// ---------------------------------------------------------------------------
// GET /api/server
// ---------------------------------------------------------------------------

/// `GET /api/server` — runtime info for `hyprfetch status` and the UI footer.
pub async fn server_info(State(state): State<AppState>) -> Json<serde_json::Value> {
    let cached = state.update_cache.lock().await;
    Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": state.started_at.elapsed().as_secs(),
        "active_tasks": state.engine.active_task_count().unwrap_or(0),
        "ws_clients": state.ws_clients.load(std::sync::atomic::Ordering::Relaxed),
        "update_available": cached.as_ref().map(|c| c.available),
        "latest_version": cached.as_ref().map(|c| c.latest.clone()),
        "quiet": state.engine.is_quiet(),
    }))
}

// ---------------------------------------------------------------------------
// GET /api/system/usage + POST /api/power/quiet|wake  (v0.4.6)
// ---------------------------------------------------------------------------

/// `GET /api/system/usage` — RAM + CPU + threads used by THIS app only.
/// Read from the kernel's own accounting for the current PID; no
/// system-wide metrics are touched. Powers the footer widget and the
/// Settings → App card.
pub async fn system_usage(State(state): State<AppState>) -> Json<serde_json::Value> {
    let usage = state.usage.sample(state.started_at.elapsed().as_secs());
    Json(serde_json::json!({
        "rss_bytes": usage.rss_bytes,
        "peak_rss_bytes": usage.peak_rss_bytes,
        "cpu_percent": (usage.cpu_percent * 100.0).round() / 100.0,
        "threads": usage.threads,
        "uptime_secs": usage.uptime_secs,
        "quiet": state.engine.is_quiet(),
    }))
}

/// `POST /api/power/quiet` — enter background (low-usage) mode: the server
/// keeps running (downloads continue) but its own periodic work wakes 10×
/// less often. The desktop-friendly way to "close" HyprFetch without
/// killing active downloads; reopen with `hyprfetch open` or the browser.
pub async fn power_quiet(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.engine.set_quiet(true);
    let usage = state.usage.sample(state.started_at.elapsed().as_secs());
    EventsRepo::new(&state.db)
        .append(None, "power.quiet", "{}")
        .ok();
    Ok(Json(serde_json::json!({
        "quiet": true,
        "pid": std::process::id(),
        "rss_bytes": usage.rss_bytes,
        "reopen": "hyprfetch open",
    })))
}

/// `POST /api/power/wake` — leave background mode (normal 1s ticks).
pub async fn power_wake(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.engine.set_quiet(false);
    EventsRepo::new(&state.db)
        .append(None, "power.wake", "{}")
        .ok();
    Ok(Json(serde_json::json!({ "quiet": false })))
}

// ---------------------------------------------------------------------------
// GET /api/update/check
// ---------------------------------------------------------------------------

/// Detect other `hyprfetch` copies on PATH (e.g. an old install.sh build in
/// `/usr/local/bin` shadowing the pacman-managed `/usr/bin` one). Users keep
/// "updating" while the stale copy keeps launching — surface them so the UI
/// can offer a one-click fix. Never fails the response.
async fn stale_copies_json() -> serde_json::Value {
    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("hyprfetch"));
    serde_json::to_value(hyprfetch_core::update::shadowed_copies(&exe).await)
        .unwrap_or(serde_json::json!([]))
}

/// `GET /api/update/check` — query the newest version from the self-hosted
/// update channel (istias.tech) and cache it. GitHub is never contacted;
/// when the channel is unreachable the response carries the updates-page
/// URL so the UI can point the user at manual steps. The response also lists
/// stale shadowing copies (see [`stale_copies_json`]).
pub async fn update_check(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let stale = stale_copies_json().await;
    // One-click update helper state (a process probe — cheap, but keep it
    // off the async reactor).
    let one_click_ready = tokio::task::spawn_blocking(hyprfetch_core::update::priv_helper_ready)
        .await
        .unwrap_or(false);
    match hyprfetch_core::update::check(&state.update_cfg).await {
        Ok(chk) => {
            let available = chk.available;
            let latest = chk.latest.clone();
            *state.update_cache.lock().await = Some(chk.clone());
            let mut v = serde_json::to_value(chk).unwrap_or(serde_json::json!({
                "current": env!("CARGO_PKG_VERSION"),
                "latest": latest,
                "available": available,
            }));
            v["stale_copies"] = stale;
            v["one_click_ready"] = serde_json::Value::Bool(one_click_ready);
            Ok(Json(v))
        }
        Err(e) => Ok(Json(serde_json::json!({
            "current": env!("CARGO_PKG_VERSION"),
            "latest": null,
            "available": false,
            "error": format!("update channel unreachable: {e}"),
            "updates_page": hyprfetch_core::update::UPDATES_PAGE_URL,
            "stale_copies": stale,
            "one_click_ready": one_click_ready,
        }))),
    }
}

// ---------------------------------------------------------------------------
// POST /api/update/apply
// ---------------------------------------------------------------------------

/// Query body for `POST /api/update/apply`.
#[derive(Debug, Deserialize, Default)]
pub struct UpdateApplyQuery {
    /// Restart the server after a successful swap (default: true).
    pub restart: Option<bool>,
}

/// `POST /api/update/apply` — download, sha256-verify, swap the binary and
/// (optionally, default) drain-pause → re-exec → auto-resume.
pub async fn update_apply(
    State(state): State<AppState>,
    Query(q): Query<UpdateApplyQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let latest = {
        let cache = state.update_cache.lock().await;
        cache
            .as_ref()
            .map(|c| c.latest.clone())
            .ok_or_else(|| ApiError::InvalidRequest("run GET /api/update/check first".into()))?
    };

    let chk = hyprfetch_core::update::UpdateCheck {
        current: env!("CARGO_PKG_VERSION").to_string(),
        latest: latest.clone(),
        available: true,
        published_at: None,
        release_url: None,
        asset: None,
        channel: None,
    };

    // Download from the project mirror → sha256-verify → swap atomically.
    // The download itself works for ANY user (streamed, resumable — no
    // total deadline); only the final swap on a system-owned install needs
    // privileges, walking the ladder: one-click helper (silent) →
    // `sudo -n` (silent) → `pkexec` (GUI password prompt when a polkit
    // agent runs) → `needs_password` (staged binary kept for the one-time
    // setup below).
    let download_result = hyprfetch_core::update::apply_with_progress(
        &state.update_cfg,
        &chk,
        hyprfetch_core::update::Escalation::NonInteractive,
        |done, total| {
            // progress lands in the daemon log every ~1 MiB
            if total > 0 && (done % (1024 * 1024)) < 64 * 1024 {
                tracing::debug!(done, total, "update download progress");
            }
        },
    )
    .await;

    let applied = match download_result {
        Ok(a) => a,
        Err(hyprfetch_core::update::UpdateError::PasswordRequired {
            staged,
            target,
            hint,
        }) => {
            // Keep the staged binary and answer needs_password so the UI
            // can offer the ONE-TIME setup (one password entry → silent
            // in-app updates forever after).
            let latest_for_stage = latest.clone();
            *state.staged_update.lock().await = Some(crate::StagedUpdate {
                staged,
                target,
                latest: latest_for_stage,
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            });
            return Ok(Json(serde_json::json!({
                "needs_password": true,
                "one_click_ready": false,
                "hint": hint,
                "manual_command": "sudo hyprfetch update",
            })));
        }
        Err(hyprfetch_core::update::UpdateError::RootNeeded { path, hint }) => {
            return Err(ApiError::InvalidRequest(format!(
                "this HyprFetch was installed in a system location ({path}) and \
                 no passwordless privilege tool answered. \
                 Update it from a terminal instead: run `sudo hyprfetch update` once. {hint}"
            )));
        }
        Err(e) => return Err(ApiError::InternalError(format!("update apply: {e}"))),
    };

    // A successful swap invalidates any pending one-click staging.
    *state.staged_update.lock().await = None;

    let restart = q.restart.unwrap_or(true);
    let restarted = if restart {
        // After a migration the NEW binary lives at applied.new_path — spawn
        // the replacement server from there (current_exe is still the old
        // location).
        trigger_restart_from(
            &state,
            applied.new_path.as_deref().map(std::path::Path::new),
        )
        .await
        .is_ok()
    } else {
        false
    };

    let stale = stale_copies_json().await;
    Ok(Json(serde_json::json!({
        "installed": applied.installed,
        "previous": applied.current,
        "sha256": applied.sha256,
        "backup": applied.backup_path,
        "escalated": applied.escalated,
        "migrated": applied.migrated,
        "new_path": applied.new_path,
        "path_fixes": applied.path_fixes,
        "system_fix_hint": applied.system_fix_hint,
        "restarting": restarted,
        "stale_copies": stale,
    })))
}

// ---------------------------------------------------------------------------
// POST /api/update/authorize  +  GET /api/update/authorize/status
// ---------------------------------------------------------------------------

/// `POST /api/update/authorize` — the ONE-TIME one-click update setup.
///
/// Requires a pending staged update (i.e. `POST /api/update/apply` answered
/// `needs_password` first). Spawns a TERMINAL WINDOW running the narrow
/// privileged setup under `sudo`: it installs the root-owned
/// `/usr/lib/hyprfetch/privileged-update` helper + a validated sudoers
/// drop-in for THIS user, then finishes the pending swap with the staged
/// binary. One password entry — every future in-app update runs silently
/// through the helper. The daemon polls for completion and restarts itself.
pub async fn update_authorize(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Exactly one authorize run at a time.
    {
        let auth = state.authorize_state.lock().await;
        if auth.running && !auth.done && auth.error.is_none() {
            return Err(ApiError::InvalidRequest(
                "an update authorization is already running — check /api/update/authorize/status"
                    .into(),
            ));
        }
    }
    let staged = {
        let s = state.staged_update.lock().await;
        s.clone().ok_or_else(|| {
            ApiError::InvalidRequest(
                "no pending update to authorize — run Check now → Install & restart first".into(),
            )
        })?
    };
    // Staging entries expire after 1 h (the temp dir may have been cleaned).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now.saturating_sub(staged.created_at) > 3600
        || !std::path::Path::new(&staged.staged).exists()
    {
        *state.staged_update.lock().await = None;
        return Err(ApiError::InvalidRequest(
            "the staged update expired — run Check now → Install & restart again".into(),
        ));
    }

    let user = hyprfetch_core::update::invoking_user().unwrap_or_else(|| "root".to_string());
    let script = hyprfetch_core::update::setup_script(
        &user,
        std::path::Path::new(&staged.staged),
        std::path::Path::new(&staged.target),
    );

    // Reset + mark running.
    {
        let mut auth = state.authorize_state.lock().await;
        *auth = crate::AuthorizeState {
            running: true,
            terminal: None,
            started_at: Some(now),
            done: false,
            restarted: false,
            error: None,
        };
    }

    // Spawn the terminal window (fire-and-forget) and watch for the swap.
    let spawned =
        tokio::task::spawn_blocking(move || hyprfetch_core::update::spawn_terminal_script(&script))
            .await
            .map_err(|e| ApiError::InternalError(format!("authorize join: {e}")))?;

    match spawned {
        Ok(terminal) => {
            state.authorize_state.lock().await.terminal = Some(terminal.clone());
            // Watcher: poll until the helper consumed the staged binary
            // (success) or the 6-minute window closes.
            let watch_state = state.clone();
            let staged_path = staged.staged.clone();
            let started = now;
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let consumed = !std::path::Path::new(&staged_path).exists();
                    let expired = now.saturating_sub(started) > 360;
                    let mut auth = watch_state.authorize_state.lock().await;
                    if consumed && hyprfetch_core::update::priv_helper_ready() {
                        auth.done = true;
                        auth.running = false;
                        drop(auth);
                        tracing::info!("one-click update setup finished — restarting daemon");
                        let restarted = trigger_restart(&watch_state).await.is_ok();
                        watch_state.authorize_state.lock().await.restarted = restarted;
                        return;
                    }
                    if expired {
                        auth.running = false;
                        auth.error = Some(
                            "timed out waiting for the terminal setup — did the password \
                             prompt complete? try again or run `sudo hyprfetch update`"
                                .into(),
                        );
                        return;
                    }
                }
            });
            Ok(Json(serde_json::json!({
                "spawned": true,
                "terminal": terminal,
                "message": format!(
                    "a {terminal} window opened — enter your password there once; \
                     the app updates and restarts by itself"
                ),
            })))
        }
        Err(e) => {
            let mut auth = state.authorize_state.lock().await;
            auth.running = false;
            auth.error = Some(e.to_string());
            Ok(Json(serde_json::json!({
                "spawned": false,
                "error": e.to_string(),
                "manual_command": "sudo hyprfetch update",
            })))
        }
    }
}

/// `GET /api/update/authorize/status` — poll for the one-time setup result.
pub async fn update_authorize_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let auth = state.authorize_state.lock().await;
    Json(serde_json::to_value(&*auth).unwrap_or(serde_json::json!({})))
}

// ---------------------------------------------------------------------------
// POST /api/update/restart
// ---------------------------------------------------------------------------

/// `POST /api/update/restart` — drain-pause active downloads, re-exec a fresh
/// server with the same arguments, then gracefully stop this process. The new
/// process auto-resumes the paused tasks on startup.
pub async fn update_restart(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    trigger_restart(&state)
        .await
        .map_err(|e| ApiError::InternalError(format!("restart: {e}")))?;
    Ok(Json(serde_json::json!({"restarting": true})))
}

// ---------------------------------------------------------------------------
// POST /api/update/stale-copies/fix
// ---------------------------------------------------------------------------

/// `POST /api/update/stale-copies/fix` — remove every non-package-owned
/// stale `hyprfetch` copy found on PATH (the classic case: an old
/// install.sh copy in `/usr/local/bin` shadowing the pacman-managed
/// `/usr/bin` build, so the old UI/CLI keeps launching). Package-owned
/// files are reported back for manual removal via the package manager.
pub async fn update_fix_stale_copies(
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let exe = std::env::current_exe()
        .map_err(|e| ApiError::InternalError(format!("current_exe: {e}")))?;
    let copies = hyprfetch_core::update::shadowed_copies(&exe).await;
    if copies.is_empty() {
        return Ok(Json(serde_json::json!({
            "removed": [], "failed": [], "owned": [],
            "message": "no stale copies found — nothing to fix",
        })));
    }
    let tool = hyprfetch_core::update::find_priv_tool_noninteractive();
    let mut removed: Vec<String> = Vec::new();
    let mut failed: Vec<serde_json::Value> = Vec::new();
    let mut owned: Vec<serde_json::Value> = Vec::new();
    for c in copies {
        if c.owned_by.is_some() {
            owned.push(serde_json::json!({
                "path": c.path,
                "owned_by": c.owned_by,
                "hint": "remove via the package manager (e.g. `sudo pacman -Rns hyprfetch-bin`)"
            }));
            continue;
        }
        match hyprfetch_core::update::remove_stale_copy(
            std::path::Path::new(&c.path),
            tool.as_ref(),
        )
        .await
        {
            Ok(()) => {
                tracing::info!(path = %c.path, "removed stale shadowing hyprfetch copy");
                removed.push(c.path);
            }
            Err(e) => failed.push(serde_json::json!({ "path": c.path, "error": e.to_string() })),
        }
    }
    let message = if removed.is_empty() && failed.is_empty() {
        "package-owned copies must be removed via the package manager".to_string()
    } else if failed.is_empty() {
        "stale copies removed — restart the server to run the fresh binary".to_string()
    } else {
        "some copies could not be removed — see 'failed' (passwordless sudo/pkexec required)"
            .to_string()
    };
    Ok(Json(serde_json::json!({
        "removed": removed, "failed": failed, "owned": owned, "message": message,
    })))
}

/// Shared restart logic: drain → spawn replacement → notify shutdown.
async fn trigger_restart(state: &AppState) -> Result<(), String> {
    trigger_restart_from(state, None).await
}

/// [`trigger_restart`] with an explicit binary — used after a migration so
/// the replacement server runs the freshly installed `~/.local/bin`
/// binary instead of the old (possibly still-shadowed) system path.
async fn trigger_restart_from(
    state: &AppState,
    exe_override: Option<&std::path::Path>,
) -> Result<(), String> {
    let args = crate::SERVE_ARGS
        .get()
        .ok_or_else(|| {
            "server was not started through `hyprfetch serve`; use `hyprfetch restart`".to_string()
        })?
        .clone();

    // 1. Drain: pause active downloads (auto-resumed by the new process).
    match state.engine.pause_all_active().await {
        Ok(n) if n > 0 => tracing::info!(tasks = n, "drained active downloads for restart"),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "drain pass failed; continuing restart"),
    }

    // 2. Spawn the replacement server, detached from this process.
    let exe = match exe_override {
        Some(p) => p.to_path_buf(),
        None => std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?,
    };
    tracing::info!(exe = %exe.display(), "spawning replacement server");
    #[allow(clippy::zombie_processes)]
    let child = {
        use std::os::unix::process::CommandExt;
        std::process::Command::new(&exe)
            .args(&args)
            .process_group(0)
            .spawn()
            .map_err(|e| format!("spawn replacement: {e}"))?
    };
    let pid = child.id();
    tracing::info!(pid, "replacement server spawned");
    std::mem::forget(child); // detached: do not reap; it outlives us

    // 3. Graceful shutdown of THIS process.
    state.shutdown.notify_waiters();
    Ok(())
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

    // -- categories & save dirs ------------------------------------------

    /// Seed `download_dir` to a fresh temp dir; returns its path string.
    async fn seed_download_dir(state: &AppState) -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dl").to_string_lossy().into_owned();
        // Keep the tempdir alive for the process lifetime (tests are short).
        std::mem::forget(dir);
        let app = router(state.clone());
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri("/api/settings")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "download_dir": path,
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        path
    }

    async fn post_task(state: &AppState, body: serde_json::Value) -> (StatusCode, String) {
        let app = router(state.clone());
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let body = body_str(res.into_body()).await;
        (status, body)
    }

    #[tokio::test]
    async fn create_task_auto_categorizes_by_extension() {
        let state = test_state();
        let base = seed_download_dir(&state).await;
        let (status, body) = post_task(
            &state,
            serde_json::json!({"urls": ["https://example.com/movie.mkv"]}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let save_path = v["tasks"][0]["save_path"].as_str().unwrap();
        let category = v["tasks"][0]["category"].as_str().unwrap();
        assert_eq!(category, "video");
        assert!(
            save_path.starts_with(&format!("{base}/video/")),
            "save_path {save_path} should live under {base}/video/"
        );
        assert!(std::path::Path::new(&format!("{base}/video")).is_dir());
    }

    #[tokio::test]
    async fn create_task_accepts_explicit_auto_category() {
        // Regression (v0.4.4): the WebUI modal's default "Auto-sort by file
        // type" sends category:"auto"; before 0.4.4 that was a 400 error.
        let state = test_state();
        let base = seed_download_dir(&state).await;
        let (status, body) = post_task(
            &state,
            serde_json::json!({"urls": ["https://example.com/song.mp3"], "category": "auto"}),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "category:auto must be accepted, got {body}"
        );
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let save_path = v["tasks"][0]["save_path"].as_str().unwrap();
        assert!(
            save_path.starts_with(&format!("{base}/music/")),
            "auto must sort .mp3 into music, got {save_path}"
        );
    }

    #[tokio::test]
    async fn create_task_direct_save_beats_category() {
        let state = test_state();
        let base = seed_download_dir(&state).await;
        let direct = format!("{base}-direct");
        let (status, body) = post_task(
            &state,
            serde_json::json!({
                "urls": ["https://example.com/clip.mp4"],
                "save_dir": direct,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let save_path = v["tasks"][0]["save_path"].as_str().unwrap();
        assert!(
            save_path.starts_with(&direct),
            "direct save must win over auto-categorization"
        );
        assert!(!save_path.contains("/video/"));
    }

    #[tokio::test]
    async fn create_task_explicit_category_and_unknown_rejected() {
        let state = test_state();
        let base = seed_download_dir(&state).await;
        let (status, body) = post_task(
            &state,
            serde_json::json!({
                "urls": ["https://example.com/whatever.bin"],
                "category": "music",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(v["tasks"][0]["save_path"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{base}/music/")));

        let (status, _) = post_task(
            &state,
            serde_json::json!({
                "urls": ["https://example.com/x.iso"],
                "category": "not-a-category",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn categorize_off_lands_in_base_dir() {
        let state = test_state();
        let base = seed_download_dir(&state).await;
        let app = router(state.clone());
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri("/api/settings")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({"categorize": "false"})).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let (status, body) = post_task(
            &state,
            serde_json::json!({"urls": ["https://example.com/song.mp3"]}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let save_path = v["tasks"][0]["save_path"].as_str().unwrap();
        assert!(
            save_path.starts_with(&format!("{base}/song.mp3")),
            "categorize=false must save into the base dir, got {save_path}"
        );
    }

    #[tokio::test]
    async fn categories_endpoint_lists_all_with_dirs() {
        let state = test_state();
        let base = seed_download_dir(&state).await;
        let app = router(state.clone());
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/categories")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let cats = v["categories"].as_array().unwrap();
        assert_eq!(cats.len(), 7);
        assert!(v["base"].as_str().unwrap().starts_with(&base));
        for c in cats {
            assert!(!c["dir"].as_str().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn category_override_is_applied_and_created() {
        let state = test_state();
        let tmp = tempfile::tempdir().unwrap();
        let music_dir = tmp.path().join("my-music").to_string_lossy().into_owned();
        std::mem::forget(tmp);
        let app = router(state.clone());
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri("/api/settings")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "category_dir_music": music_dir,
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(
            std::path::Path::new(&music_dir).is_dir(),
            "override dir is auto-created"
        );
    }

    #[tokio::test]
    async fn settings_mask_github_token() {
        let state = test_state();
        let app = router(state.clone());
        // Set a token through the API.
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri("/api/settings")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "github_token": "super-secret-value-1234567890",
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        assert!(
            !body.contains("super-secret-value"),
            "raw token must not leak"
        );
        assert!(body.contains("github_token_set"));
        assert!(body.contains("true"));

        // GET also masks.
        let app = router(state.clone());
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/settings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_str(res.into_body()).await;
        assert!(!body.contains("super-secret-value"));
        assert!(body.contains("github_token_set"));
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
            source: "app".into(),
            media_meta: None,
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

    // -- pause/resume race -------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn resume_immediately_after_pause_does_not_strand_task() {
        // Regression: pausing is asynchronous (workers stop, offsets flush,
        // the coordinator self-reaps). The resume route used to flip the DB
        // row straight to `downloading` and then call engine.start(), which
        // rejected a `downloading` row unconditionally — the error was
        // swallowed and the task was stranded in `downloading` forever with
        // no coordinator attached. The route now retries through the
        // wind-down window and rolls back to `paused` (409) on persistent
        // failure; the engine accepts a stranded `downloading` row as
        // restartable. This test drives pause→resume with ZERO delay, the
        // exact race window, and requires the task to finish.
        use hyprfetch_core::{Engine, SsrfPolicy};
        use std::sync::Arc;
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

        const TOTAL: i64 = 8 * 1024 * 1024;
        const SEGS: i64 = 4;

        // Dynamic Range responder: after a pause mid-segment the resumed
        // workers request ARBITRARY byte ranges, so the mock must slice the
        // body per request instead of matching fixed segment boundaries.
        struct ServeRange(Arc<Vec<u8>>);
        impl Respond for ServeRange {
            fn respond(&self, req: &wiremock::Request) -> ResponseTemplate {
                let total = self.0.len();
                let range = req
                    .headers
                    .get("range")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                if range.is_empty() || !range.starts_with("bytes=") {
                    return ResponseTemplate::new(200)
                        .insert_header("content-length", total.to_string())
                        .insert_header("accept-ranges", "bytes")
                        .insert_header("etag", "\"resume-race-etag\"");
                }
                let spec = range.trim_start_matches("bytes=");
                let (a, b) = spec.split_once('-').unwrap_or(("", ""));
                let (start, end) = if a.is_empty() {
                    // suffix form: bytes=-N
                    let n: usize = b.parse().unwrap_or(total);
                    (total.saturating_sub(n), total - 1)
                } else {
                    let s: usize = a.parse().unwrap_or(0);
                    let e: usize = b.parse().unwrap_or(total - 1);
                    (s, e.min(total - 1))
                };
                let slice = self.0[start..=end].to_vec();
                ResponseTemplate::new(206)
                    .insert_header("content-range", format!("bytes {start}-{end}/{total}"))
                    .insert_header("content-length", slice.len().to_string())
                    .set_body_bytes(slice)
            }
        }

        let server = MockServer::start().await;
        let body: Arc<Vec<u8>> = Arc::new((0..TOTAL as usize).map(|i| (i % 251) as u8).collect());
        Mock::given(method("HEAD"))
            .respond_with(ServeRange(Arc::clone(&body)))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ServeRange(body))
            .mount(&server)
            .await;

        let db = hyprfetch_db::open_in_memory().unwrap();
        let engine = Engine::with_ssrf_policy(
            db.clone(),
            SsrfPolicy {
                block_private: false,
            },
        );
        // Throttle to 2 MiB/s so the 8 MiB download spans ~4s and the
        // pause→resume race window is wide and deterministic.
        engine.set_qos(true, 2 * 1024 * 1024);
        let state = crate::AppState::with_defaults(db.clone(), std::sync::Arc::new(engine));
        let app = router(state.clone());

        let id = uuid::Uuid::now_v7().to_string();
        let now = 0i64;
        let row = TaskRow {
            id: id.clone(),
            url: format!("{}/file.bin", server.uri()),
            filename: "file.bin".into(),
            save_path: "/tmp/hyprfetch-resume-race.bin".into(),
            total_bytes: Some(TOTAL),
            downloaded_bytes: 0,
            state: TaskState::Queued,
            etag: None,
            last_modified: None,
            accept_ranges: false,
            segments_requested: SEGS,
            qos_override: None,
            extra_headers: None,
            error_message: None,
            created_at: now,
            updated_at: now,
            completed_at: None,
            source: "app".into(),
            media_meta: None,
        };
        TasksRepo::new(&db).insert(&row).unwrap();
        state.engine.start(&id).await.unwrap();

        // Wait for real progress AND the downloading state before hitting
        // the race window.
        for _ in 0..400 {
            let repo_row = TasksRepo::new(&db).get(&id).ok().flatten();
            let bytes = repo_row.as_ref().map(|r| r.downloaded_bytes).unwrap_or(0);
            let st = repo_row
                .map(|r| r.state.as_str().to_string())
                .unwrap_or_default();
            if bytes > 0 && st == "downloading" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }

        // Pause, then resume with NO delay — the exact race.
        let pause = app
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
        assert_eq!(pause.status(), StatusCode::OK);

        let resume = app
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
        // Either the engine (re)started (200) or it honestly rolled back to
        // paused with 409 — both are recoverable. A silent 200 with a
        // stranded task is the bug.
        assert!(
            resume.status() == StatusCode::OK || resume.status() == StatusCode::CONFLICT,
            "resume must be 200 or 409, got {}",
            resume.status()
        );

        // The task must reach `complete` (resuming again if it was rolled
        // back). It must NEVER sit in `downloading` without progressing.
        let mut complete = false;
        for _ in 0..240 {
            let st = db_state(&db, &id);
            match st.as_str() {
                "complete" => {
                    complete = true;
                    break;
                }
                "paused" => {
                    let _ = app
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
                }
                _ => {}
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        let final_row = TasksRepo::new(&db).get(&id).ok().flatten();
        assert!(
            complete,
            "task must complete after resume; last state was {}, error {:?}",
            db_state(&db, &id),
            final_row.and_then(|r| r.error_message)
        );
    }

    /// Current state string of a task straight from the DB.
    fn db_state(db: &std::sync::Arc<std::sync::Mutex<rusqlite::Connection>>, id: &str) -> String {
        TasksRepo::new(db)
            .get(id)
            .ok()
            .flatten()
            .map(|r| r.state.as_str().to_string())
            .unwrap_or_else(|| "missing".into())
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

    // -- /api/inspect -------------------------------------------------------

    #[tokio::test]
    async fn inspect_rejects_non_http_scheme() {
        let app = router(test_state());
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/inspect")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "url": "file:///etc/passwd",
                        }))
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
    async fn inspect_blocks_private_hosts_by_default() {
        // Default SSRF policy blocks loopback targets — same protection as
        // real downloads, so the confirm dialog can never be tricked.
        let app = router(test_state());
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/inspect")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "url": "http://127.0.0.1:9/secret.bin",
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        let body = body_str(res.into_body()).await;
        assert!(body.contains("ssrf_blocked"), "got: {body}");
    }

    #[tokio::test]
    async fn inspect_resolves_save_path_without_creating_task() {
        // Happy path against a real HTTP endpoint is covered live in the API
        // battery; here we verify that a task is NOT created on inspect.
        let state = test_state();
        let before = list_task_count(&state);
        let _ = before; // only the side-effect matters below
        let app = router(state.clone());
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/inspect")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "url": "http://127.0.0.1:9/x.bin",
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        // 403 (SSRF) — but crucially NO task row was created either way.
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        let after = list_task_count(&state);
        assert_eq!(before, after, "inspect must never create tasks");
    }

    fn list_task_count(state: &AppState) -> usize {
        hyprfetch_db::TasksRepo::new(&state.db)
            .list_by_state(None)
            .unwrap()
            .len()
    }

    // -- /api/tasks/:id/open + /api/tasks/:id/reveal -------------------------

    #[tokio::test]
    async fn open_and_reveal_missing_task_return_404() {
        let app = router(test_state());
        for uri in ["/api/tasks/no-such-id/open", "/api/tasks/no-such-id/reveal"] {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::NOT_FOUND, "uri: {uri}");
        }
    }

    #[tokio::test]
    async fn open_task_requires_finished_download() {
        let state = test_state();
        let path = "/tmp/hyprfetch-open-partial.bin";
        std::fs::write(path, b"partial").unwrap();
        let id = seed_raw_task(&state, TaskState::Paused, path);

        let res = router(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/tasks/{id}/open"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        let body = body_str(res.into_body()).await;
        assert!(body.contains("not finished"), "got: {body}");
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn open_task_requires_file_on_disk() {
        let state = test_state();
        let id = seed_raw_task(&state, TaskState::Complete, "/tmp/hyprfetch-open-gone.bin");
        std::fs::remove_file("/tmp/hyprfetch-open-gone.bin").ok();

        let res = router(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/tasks/{id}/open"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let body = body_str(res.into_body()).await;
        assert!(body.contains("not found on disk"), "got: {body}");
    }

    #[tokio::test]
    async fn open_and_reveal_launch_opener_for_finished_task() {
        // Single test (env var is process-global): HYPRFETCH_FILE_OPENER
        // points at /bin/true so no GUI is launched in CI.
        std::env::set_var("HYPRFETCH_FILE_OPENER", "/bin/true");

        let state = test_state();
        let dir = std::env::temp_dir().join("hyprfetch-open-live");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("done.bin");
        std::fs::write(&file, b"complete").unwrap();
        let path = file.to_str().unwrap();
        let id = seed_raw_task(&state, TaskState::Complete, path);

        let app = router(state);
        // open → the file itself
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/tasks/{id}/open"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        assert!(body.contains("\"opened\":true"), "got: {body}");
        assert!(body.contains(path), "got: {body}");

        // reveal → the containing folder
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/tasks/{id}/reveal"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        assert!(body.contains("\"what\":\"folder\""), "got: {body}");
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn reveal_task_missing_folder_returns_400() {
        let state = test_state();
        let id = seed_raw_task(&state, TaskState::Error, "/tmp/hyprfetch-no-such-dir/x.bin");
        std::fs::remove_dir_all("/tmp/hyprfetch-no-such-dir").ok();

        let res = router(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/tasks/{id}/reveal"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let body = body_str(res.into_body()).await;
        assert!(body.contains("not found on disk"), "got: {body}");
    }

    // -- stale copies + update responses -----------------------------------

    #[tokio::test]
    async fn stale_copies_fix_is_idempotent_and_shaped_for_the_ui() {
        let app = router(test_state());
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/update/stale-copies/fix")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(v.get("removed").and_then(|r| r.as_array()).is_some());
        assert!(v.get("failed").and_then(|r| r.as_array()).is_some());
        assert!(v.get("owned").and_then(|r| r.as_array()).is_some());
        assert!(v.get("message").and_then(|m| m.as_str()).is_some());
    }

    #[tokio::test]
    async fn update_check_response_includes_stale_copies_key() {
        // The channel may or may not be reachable in CI — either way the
        // payload must carry the stale_copies key for the UI banner.
        let app = router(test_state());
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/update/check")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(
            v.get("stale_copies").and_then(|s| s.as_array()).is_some(),
            "stale_copies key missing: {body}"
        );
    }
    // -- v0.4.6: filename sniffing / theme settings / open-folder / usage /
    //    power --------------------------------------------------------------

    fn json_request(method: Method, uri: &str, payload: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(payload.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn create_task_strips_query_from_filename() {
        // `…/images?q=tbn:ANd9Gc…` used to store the whole query tail as the
        // filename (and land in `other/`). It must now store just `images`.
        let app = router(test_state());
        let res = app
            .oneshot(json_request(
                Method::POST,
                "/api/tasks",
                serde_json::json!({"urls": ["https://example.com/images?q=tbn:ANd9GcQ"]}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = body_str(res.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let filename = v["tasks"][0]["filename"].as_str().unwrap();
        assert_eq!(
            filename, "images",
            "query string must not leak into the name"
        );
    }

    #[tokio::test]
    async fn create_task_percent_decodes_filename() {
        let app = router(test_state());
        let res = app
            .oneshot(json_request(
                Method::POST,
                "/api/tasks",
                serde_json::json!({"urls": ["https://example.com/my%20photo.png"]}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body = body_str(res.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["tasks"][0]["filename"], "my photo.png");
    }

    #[tokio::test]
    async fn inspect_sniffs_extension_from_content_type() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-length", "123")
                    .insert_header("accept-ranges", "bytes")
                    .insert_header("content-type", "image/jpeg"),
            )
            .mount(&server)
            .await;

        // Pin the base dir so the pictures path is deterministic. SSRF is
        // disabled so the engine can probe the loopback mock server.
        let base = tempfile::tempdir().unwrap();
        let base_dir = base.path().join("dl").to_string_lossy().into_owned();
        let db = hyprfetch_db::open_in_memory().unwrap();
        let engine = std::sync::Arc::new(hyprfetch_core::Engine::with_ssrf_policy(
            db.clone(),
            hyprfetch_core::SsrfPolicy {
                block_private: false,
            },
        ));
        let state = crate::AppState::with_defaults(db, engine);
        {
            let s = SettingsRepo::new(&state.db);
            s.set(SET_DOWNLOAD_DIR, &base_dir).unwrap();
        }
        let app = router(state);

        let res = app
            .oneshot(json_request(
                Method::POST,
                "/api/inspect",
                serde_json::json!({"url": format!("{}/images?q=tbn:ANd9GcQ", server.uri())}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["filename"], "images.jpg", "sniffed from Content-Type");
        assert_eq!(v["category"], "pictures");
        assert_eq!(v["content_type"], "image/jpeg");
        assert_eq!(v["save_path"], format!("{base_dir}/pictures/images.jpg"));
    }

    #[tokio::test]
    async fn patch_settings_validates_new_ui_keys() {
        let app = router(test_state());
        let app1 = app.clone();
        let app2 = app;

        // Invalid theme style → 400.
        let res = app1
            .oneshot(json_request(
                Method::PATCH,
                "/api/settings",
                serde_json::json!({"ui_theme_style": "neon-rainbow"}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        // Valid values round-trip.
        let res = app2
            .oneshot(json_request(
                Method::PATCH,
                "/api/settings",
                serde_json::json!({
                    "ui_theme_style": "ocean",
                    "ui_theme_mode": "light",
                    "show_resource_usage": "false",
                    "keep_alive_in_background": "false"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ui_theme_style"], "ocean");
        assert_eq!(v["ui_theme_mode"], "light");
        assert_eq!(v["show_resource_usage"], "false");
        assert_eq!(v["keep_alive_in_background"], "false");
    }

    #[tokio::test]
    async fn open_folder_launches_file_manager_for_save_dirs() {
        // The env var is process-global; the existing GO/Open test uses the
        // same value, so setting it here is race-free.
        std::env::set_var("HYPRFETCH_FILE_OPENER", "/bin/true");

        let base = tempfile::tempdir().unwrap();
        let base_dir = base.path().join("dl").to_string_lossy().into_owned();
        std::fs::create_dir_all(base.path().join("dl/pictures")).unwrap();

        let state = test_state();
        {
            let s = SettingsRepo::new(&state.db);
            s.set(SET_DOWNLOAD_DIR, &base_dir).unwrap();
        }
        let app = router(state);

        // A save folder is allowed.
        let res = app
            .oneshot(json_request(
                Method::POST,
                "/api/open-folder",
                serde_json::json!({"path": base.path().join("dl/pictures")}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["opened"], true);
        assert_eq!(v["opener"], "/bin/true");

        // Outside the save folders → refused.
        let app2 = router(test_state());
        let res = app2
            .oneshot(json_request(
                Method::POST,
                "/api/open-folder",
                serde_json::json!({"path": "/etc"}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        // Missing folder → 400, not a crash.
        let app3 = router(test_state());
        let res = app3
            .oneshot(json_request(
                Method::POST,
                "/api/open-folder",
                serde_json::json!({"path": "/no/such/dir/hyprfetch"}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn system_usage_reports_process_stats() {
        let app = router(test_state());
        let res = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/system/usage")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(
            v["rss_bytes"].as_u64().unwrap() > 0,
            "rss must be positive: {body}"
        );
        assert!(v["threads"].as_u64().unwrap() > 0);
        assert!(v["cpu_percent"].as_f64().unwrap() >= 0.0);
        assert!(v["quiet"].is_boolean());
    }

    #[tokio::test]
    async fn power_quiet_then_wake_round_trip() {
        let app = router(test_state());

        // quiet on
        let res = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/power/quiet",
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["quiet"], true);
        assert!(v["rss_bytes"].as_u64().unwrap() > 0);

        // server_info reports it
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/server")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_str(res.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["quiet"], true);

        // wake up
        let res = app
            .oneshot(json_request(
                Method::POST,
                "/api/power/wake",
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_str(res.into_body()).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["quiet"], false);
    }
}
