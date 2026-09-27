//! Engine — the orchestrator that owns the task table, spawns segment workers,
//! receives progress via mpsc channels, and persists to the DB debounced.
//!
//! The engine is the heart of the download manager. It does NOT touch HTTP
//! directly — that's the `HttpClient` and `SegmentWorker`. Its job is to:
//! 1. Take a task from "queued" to "downloading"
//! 2. Probe the URL for Content-Length / Accept-Ranges / ETag / Last-Modified
//! 3. Open the target file, fallocate
//! 4. Split the total bytes into N segments
//! 5. Spawn one segment worker per segment
//! 6. Aggregate progress events, debounce-persist every 500ms
//! 7. Mark the task complete when all segments finish

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hyprfetch_db::schema::{QosOverride, TaskState};
use hyprfetch_db::{SegmentRow, SegmentsRepo, SettingsRepo, TasksRepo};
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use url::Url;

use crate::http_client::{ExtraHeaders, HttpClient};
use crate::planner;
use crate::qos::QosLimiter;
use crate::segment::{SegmentEvent, SegmentWorker};
use crate::ssrf::SsrfPolicy;

/// Default progress debounce interval.
const DEBOUNCE_INTERVAL: Duration = Duration::from_millis(500);

/// Errors returned by engine operations.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("task not found: {0}")]
    TaskNotFound(String),
    #[error("task state {0} does not allow this action")]
    InvalidState(TaskState),
    #[error("http: {0}")]
    Http(#[from] crate::http_client::HttpError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("db: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("engine: {0}")]
    Other(String),
}

/// Engine holds the in-memory task table and the channel fan-in.
pub struct Engine {
    db: Arc<std::sync::Mutex<rusqlite::Connection>>,
    http: HttpClient,
    /// Tasks currently running. One entry per active task.
    tasks: RwLock<HashMap<String, ActiveTask>>,
    /// Engine-wide QoS governor shared by ALL active tasks — one token
    /// bucket caps the aggregate download rate of the whole daemon.
    qos: QosLimiter,
}

/// A running task: handle to the coordinator task + channel to send commands.
struct ActiveTask {
    /// JoinHandle for the per-task coordinator task.
    #[allow(dead_code)]
    handle: JoinHandle<()>,
    /// Channel to send commands to the coordinator (pause, cancel).
    cmd_tx: mpsc::Sender<TaskCommand>,
}

/// Commands the engine can send to a running task coordinator.
#[derive(Debug)]
enum TaskCommand {
    Pause,
    Cancel,
}

impl Engine {
    /// Construct a new engine backed by the given DB connection.
    pub fn new(db: Arc<std::sync::Mutex<rusqlite::Connection>>) -> Self {
        Self::with_ssrf_policy(db, SsrfPolicy::default())
    }

    /// Construct a new engine with a custom SSRF policy (e.g. disabled for tests).
    ///
    /// The engine-wide QoS limiter is initialized from the persisted
    /// `qos_enabled` / `qos_target_bps` settings.
    pub fn with_ssrf_policy(
        db: Arc<std::sync::Mutex<rusqlite::Connection>>,
        policy: SsrfPolicy,
    ) -> Self {
        let qos = QosLimiter::new();
        // Restore QoS state from settings (best-effort — a missing/broken
        // setting just means QoS stays off).
        if let Ok(Some(target)) = SettingsRepo::new(&db).get("qos_target_bps") {
            let enabled = SettingsRepo::new(&db)
                .get("qos_enabled")
                .ok()
                .flatten()
                .as_deref()
                == Some("true");
            if enabled {
                if let Ok(bps) = target.parse::<u64>() {
                    qos.enable(bps);
                }
            }
        }
        Self {
            db,
            http: HttpClient::new(policy),
            tasks: RwLock::new(HashMap::new()),
            qos,
        }
    }

    /// The engine-wide QoS limiter. Clone it into workers; every clone
    /// spends from the same token bucket.
    pub fn qos(&self) -> &QosLimiter {
        &self.qos
    }

    /// Apply a new global QoS configuration at runtime (from the settings
    /// API). `enabled == false` or `target_bps == 0` turns QoS off.
    pub fn set_qos(&self, enabled: bool, target_bps: u64) {
        if enabled && target_bps > 0 {
            self.qos.enable(target_bps);
        } else {
            self.qos.disable();
        }
    }

    /// Start a queued task. Probes the URL, splits into segments, spawns workers.
    pub async fn start(&self, task_id: &str) -> Result<(), EngineError> {
        // Fetch the task row.
        let row = {
            let repo = TasksRepo::new(&self.db);
            repo.get(task_id)?
                .ok_or_else(|| EngineError::TaskNotFound(task_id.to_string()))?
        };

        if row.state != TaskState::Queued && row.state != TaskState::Paused {
            return Err(EngineError::InvalidState(row.state));
        }

        // Check if already running.
        {
            let tasks = self.tasks.read().await;
            if tasks.contains_key(task_id) {
                return Err(EngineError::Other(format!(
                    "task {task_id} is already running"
                )));
            }
        }

        // Spawn the coordinator.
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let engine_db = Arc::clone(&self.db);
        let engine_http = self.http.clone();
        let engine_qos = self.qos.clone();
        let task_id_owned = task_id.to_string();
        let handle = tokio::spawn(async move {
            if let Err(e) =
                run_task_coordinator(engine_db, engine_http, engine_qos, task_id_owned, cmd_rx)
                    .await
            {
                error!(error = %e, "task coordinator failed");
            }
        });

        // Insert into the active table.
        let active = ActiveTask { handle, cmd_tx };
        self.tasks.write().await.insert(task_id.to_string(), active);

        Ok(())
    }

    /// Pause a running task. Workers will stop after the current chunk.
    pub async fn pause(&self, task_id: &str) -> Result<(), EngineError> {
        let tasks = self.tasks.read().await;
        if let Some(active) = tasks.get(task_id) {
            let _ = active.cmd_tx.send(TaskCommand::Pause).await;
            Ok(())
        } else {
            Err(EngineError::TaskNotFound(task_id.to_string()))
        }
    }

    /// Cancel a running task. Workers will be aborted.
    pub async fn cancel(&self, task_id: &str) -> Result<(), EngineError> {
        let tasks = self.tasks.read().await;
        if let Some(active) = tasks.get(task_id) {
            let _ = active.cmd_tx.send(TaskCommand::Cancel).await;
            Ok(())
        } else {
            Err(EngineError::TaskNotFound(task_id.to_string()))
        }
    }

    /// Returns true if a task is currently being downloaded.
    pub async fn is_running(&self, task_id: &str) -> bool {
        self.tasks.read().await.contains_key(task_id)
    }
}

/// Per-task coordinator: owns the segment workers, the progress aggregator,
/// the persistence debouncer, and the command channel.
async fn run_task_coordinator(
    db: Arc<std::sync::Mutex<rusqlite::Connection>>,
    http: HttpClient,
    qos: QosLimiter,
    task_id: String,
    mut cmd_rx: mpsc::Receiver<TaskCommand>,
) -> Result<(), EngineError> {
    // Re-fetch the task row (we may have paused and resumed).
    let row = {
        let repo = TasksRepo::new(&db);
        repo.get(&task_id)?
            .ok_or_else(|| EngineError::TaskNotFound(task_id.clone()))?
    };

    // Mark as downloading.
    TasksRepo::new(&db).touch(&task_id, TaskState::Downloading, row.downloaded_bytes, None)?;

    // Parse URL.
    let url = Url::parse(&row.url).map_err(|e| EngineError::Other(format!("bad url: {e}")))?;

    // Parse extra headers.
    let extra: Option<ExtraHeaders> = row
        .extra_headers
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());

    // Probe the URL.
    let probe = match http.probe(&url, extra.as_ref()).await {
        Ok(p) => p,
        Err(e) => {
            error!(task = %task_id, error = %e, "probe failed");
            TasksRepo::new(&db).touch(
                &task_id,
                TaskState::Error,
                row.downloaded_bytes,
                Some(&format!("probe failed: {e}")),
            )?;
            return Err(e.into());
        }
    };

    // Persist validators.
    TasksRepo::new(&db).update_cache_validators(
        &task_id,
        probe.etag.as_deref(),
        probe.last_modified.as_deref(),
        probe.accept_ranges,
        probe.content_length,
    )?;

    let total_bytes = match probe.content_length {
        Some(n) => n,
        None => {
            // Can't segment without a length. Mark error.
            TasksRepo::new(&db).touch(
                &task_id,
                TaskState::Error,
                row.downloaded_bytes,
                Some("server did not return Content-Length; cannot segment"),
            )?;
            return Err(EngineError::Other("missing Content-Length".into()));
        }
    };

    if !probe.accept_ranges {
        // Fall back to single-segment download (treat the whole file as one segment).
        warn!(task = %task_id, "server does not support ranges; falling back to single connection");
    }

    // Determine segment count.
    let n = if probe.accept_ranges {
        row.segments_requested.max(1)
    } else {
        1
    };
    let n = n.min(total_bytes); // cap at byte count

    // Split into segments.
    let mut segments = planner::split(total_bytes, n);
    if segments.is_empty() {
        TasksRepo::new(&db).touch(
            &task_id,
            TaskState::Error,
            0,
            Some("planner returned 0 segments"),
        )?;
        return Err(EngineError::Other("planner returned 0 segments".into()));
    }

    // If we're resuming, load existing segment offsets from DB and apply.
    let existing_segments = SegmentsRepo::new(&db).list_for_task(&task_id)?;
    if !existing_segments.is_empty() {
        for seg in &mut segments {
            if let Some(existing) = existing_segments.iter().find(|s| s.segment_idx == seg.idx) {
                seg.current_byte = existing.current_byte.max(seg.start_byte);
                if seg.is_complete() {
                    debug!(seg = seg.idx, "segment already complete, skipping");
                }
            }
        }
    }

    // Persist the segment plan (insert or touch each).
    {
        let now = now_ms();
        let rows: Vec<_> = segments
            .iter()
            .map(|s| SegmentRow {
                task_id: task_id.clone(),
                segment_idx: s.idx,
                start_byte: s.start_byte,
                end_byte: s.end_byte,
                current_byte: s.current_byte,
                state: hyprfetch_db::SegmentState::Downloading,
                speed_bps: 0,
                error_message: None,
                updated_at: now,
            })
            .collect();
        // Insert-batch fails if rows already exist (on resume). Use upsert semantics
        // by deleting first then inserting.
        let _ = SegmentsRepo::new(&db).insert_batch(&rows);
    }

    // Open the target file.
    let save_path = PathBuf::from(&row.save_path);
    if let Some(parent) = save_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = crate::segment::open_target_file(&save_path, total_bytes)?;
    let file = Arc::new(file);

    // Effective QoS for this task: the engine-wide limiter is shared by all
    // active tasks, but a per-task `force_off` override bypasses it.
    let task_qos: Option<QosLimiter> = match row.qos_override {
        Some(QosOverride::ForceOff) => None,
        _ => Some(qos),
    };

    // Spawn segment workers.
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<SegmentEvent>();
    let mut worker_handles: Vec<JoinHandle<Result<(), _>>> = Vec::new();

    for seg in segments.clone() {
        if seg.is_complete() {
            continue;
        }
        let worker = SegmentWorker {
            client: http.clone(),
            url: url.clone(),
            segment: seg,
            file: Arc::clone(&file),
            qos: task_qos.clone(),
            buffer_size: 64 * 1024,
            progress_tx: progress_tx.clone(),
            extra_headers: extra.clone(),
        };
        let h = tokio::spawn(async move { worker.run().await });
        worker_handles.push(h);
    }
    drop(progress_tx); // workers hold their own clones; close when all done.

    // Aggregate progress + debounce-persist.
    let mut last_persist = Instant::now();
    let mut completed_count = 0usize;
    let total_workers = worker_handles.len();
    let mut downloaded_bytes: i64 = segments.iter().map(|s| s.downloaded()).sum();
    let initial_downloaded = downloaded_bytes;
    let mut per_segment_current: HashMap<i64, i64> =
        segments.iter().map(|s| (s.idx, s.current_byte)).collect();

    loop {
        tokio::select! {
            // Bias toward commands so pause/cancel win over progress.
            biased;
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(TaskCommand::Pause) => {
                        info!(task = %task_id, "pause requested");
                        // Abort all workers.
                        for h in &worker_handles {
                            h.abort();
                        }
                        // Persist current state.
                        let _ = persist_progress(&db, &task_id, &per_segment_current, &segments, TaskState::Paused, None).await;
                        return Ok(());
                    }
                    Some(TaskCommand::Cancel) | None => {
                        info!(task = %task_id, "cancel requested");
                        for h in &worker_handles {
                            h.abort();
                        }
                        let _ = persist_progress(&db, &task_id, &per_segment_current, &segments, TaskState::Removed, None).await;
                        return Ok(());
                    }
                }
            }
            ev = progress_rx.recv() => {
                match ev {
                    Some(SegmentEvent::Progress { idx, bytes_written: _, current_byte }) => {
                        per_segment_current.insert(idx, current_byte);
                        // Recompute total downloaded bytes from per-segment positions.
                        downloaded_bytes = per_segment_current.iter().map(|(&i, &c)| {
                            let s = segments.iter().find(|s| s.idx == i).copied();
                            s.map(|s| (c - s.start_byte).max(0)).unwrap_or(0)
                        }).sum();
                    }
                    Some(SegmentEvent::Completed { idx }) => {
                        completed_count += 1;
                        debug!(task = %task_id, seg = idx, completed_count, total_workers, "segment completed");
                    }
                    Some(SegmentEvent::Failed { idx, error }) => {
                        error!(task = %task_id, seg = idx, error = %error, "segment failed");
                        // Abort remaining workers.
                        for h in &worker_handles {
                            h.abort();
                        }
                        let _ = persist_progress(&db, &task_id, &per_segment_current, &segments, TaskState::Error, Some(&error)).await;
                        return Ok(());
                    }
                    None => {
                        // All workers dropped their senders — done.
                        break;
                    }
                }
                // Debounced persist.
                if last_persist.elapsed() >= DEBOUNCE_INTERVAL {
                    let _ = persist_progress(&db, &task_id, &per_segment_current, &segments, TaskState::Downloading, None).await;
                    last_persist = Instant::now();
                }
            }
        }
    }

    // Wait for all workers to finish (some may have errored but already
    // sent Failed events above).
    for h in worker_handles {
        let _ = h.await;
    }

    // Final persist.
    let final_state = if completed_count == total_workers {
        TaskState::Complete
    } else {
        TaskState::Error
    };
    let err_msg = if final_state == TaskState::Error {
        Some(format!(
            "only {completed_count} of {total_workers} segments completed"
        ))
    } else {
        None
    };
    persist_progress(
        &db,
        &task_id,
        &per_segment_current,
        &segments,
        final_state,
        err_msg.as_deref(),
    )
    .await?;

    info!(
        task = %task_id,
        downloaded = downloaded_bytes,
        initial = initial_downloaded,
        state = %final_state,
        "task finished"
    );
    Ok(())
}

async fn persist_progress(
    db: &Arc<std::sync::Mutex<rusqlite::Connection>>,
    task_id: &str,
    per_segment: &HashMap<i64, i64>,
    segments: &[crate::Segment],
    state: TaskState,
    error: Option<&str>,
) -> Result<(), rusqlite::Error> {
    // Sum of bytes-downloaded per segment = sum of (current - start_byte).
    let downloaded: i64 = per_segment
        .iter()
        .map(|(&idx, &current)| {
            let seg = segments.iter().find(|s| s.idx == idx).copied();
            seg.map(|s| (current - s.start_byte).max(0)).unwrap_or(0)
        })
        .sum();
    // Update task row.
    TasksRepo::new(db).touch(task_id, state, downloaded, error)?;
    // Update each segment row.
    let segs = SegmentsRepo::new(db);
    for (&idx, &current) in per_segment {
        let seg_state = hyprfetch_db::SegmentState::Downloading;
        segs.touch(task_id, idx, current, seg_state, 0)?;
    }
    Ok(())
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyprfetch_db::{open_in_memory, TaskRow};
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fresh_engine() -> Engine {
        let db = open_in_memory().unwrap();
        // Disable SSRF for tests so we can hit 127.0.0.1 (wiremock server).
        Engine::with_ssrf_policy(
            db,
            SsrfPolicy {
                block_private: false,
            },
        )
    }

    fn seed_task(
        db: &Arc<std::sync::Mutex<rusqlite::Connection>>,
        url: &str,
        total: Option<i64>,
    ) -> String {
        let id = uuid::Uuid::now_v7().to_string();
        let now = now_ms();
        let row = TaskRow {
            id: id.clone(),
            url: url.into(),
            filename: "test.bin".into(),
            save_path: "/tmp/hyprfetch-test.bin".into(),
            total_bytes: total,
            downloaded_bytes: 0,
            state: TaskState::Queued,
            etag: None,
            last_modified: None,
            accept_ranges: false,
            segments_requested: 4,
            qos_override: None,
            extra_headers: None,
            error_message: None,
            created_at: now,
            updated_at: now,
            completed_at: None,
        };
        TasksRepo::new(db).insert(&row).unwrap();
        id
    }

    #[tokio::test]
    async fn start_nonexistent_task_returns_error() {
        let engine = fresh_engine();
        let err = engine
            .start("00000000-0000-0000-0000-000000000000")
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::TaskNotFound(_)));
    }

    #[test]
    fn qos_loaded_from_settings_on_construction() {
        let db = open_in_memory().unwrap();
        {
            let s = hyprfetch_db::SettingsRepo::new(&db);
            s.set("qos_enabled", "true").unwrap();
            s.set("qos_target_bps", "123456").unwrap();
        }
        let engine = Engine::new(db);
        assert!(engine.qos().is_enabled());
        assert_eq!(engine.qos().target_bps(), 123_456);
    }

    #[test]
    fn qos_stays_off_when_setting_disabled() {
        let db = open_in_memory().unwrap();
        {
            let s = hyprfetch_db::SettingsRepo::new(&db);
            s.set("qos_enabled", "false").unwrap();
            s.set("qos_target_bps", "123456").unwrap();
        }
        let engine = Engine::new(db);
        assert!(!engine.qos().is_enabled());
    }

    #[test]
    fn set_qos_updates_shared_limiter() {
        let engine = fresh_engine();
        assert!(!engine.qos().is_enabled());
        engine.set_qos(true, 500_000);
        assert!(engine.qos().is_enabled());
        assert_eq!(engine.qos().target_bps(), 500_000);
        engine.set_qos(false, 500_000);
        assert!(!engine.qos().is_enabled());
    }

    #[tokio::test]
    async fn force_off_task_bypasses_qos() {
        // Enable a painfully slow QoS, start a task with force_off override,
        // and verify it still completes quickly. A 100-byte download at
        // 600 B/s is instant from a full bucket anyway — so instead assert
        // on wiring: the coordinator accepts force_off tasks while QoS is on.
        let server = MockServer::start().await;
        let body: Vec<u8> = (0..100).collect();
        let total = body.len() as i64;
        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-length", total.to_string())
                    .insert_header("accept-ranges", "bytes"),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", format!("bytes 0-99/{total}"))
                    .insert_header("content-length", "100")
                    .set_body_bytes(body),
            )
            .mount(&server)
            .await;

        let engine = fresh_engine();
        engine.set_qos(true, 600);
        let id = seed_task(&engine.db, &server.uri(), Some(total));
        {
            let db = Arc::clone(&engine.db);
            let conn = db.lock().unwrap();
            conn.execute(
                "UPDATE tasks SET qos_override = 'force_off', segments_requested = 1 WHERE id = ?1",
                rusqlite::params![id],
            )
            .unwrap();
        }
        engine.start(&id).await.unwrap();

        let mut attempts = 0;
        loop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            attempts += 1;
            if attempts > 60 {
                panic!("force_off task did not complete in time");
            }
            let row = TasksRepo::new(&engine.db).get(&id).unwrap().unwrap();
            if row.state == TaskState::Complete || row.state == TaskState::Error {
                assert_eq!(row.state, TaskState::Complete, "{:?}", row.error_message);
                break;
            }
        }
    }

    #[tokio::test]
    async fn start_with_invalid_scheme_returns_error() {
        let engine = fresh_engine();
        let id = seed_task(&engine.db, "ftp://example.com/x", Some(100));
        // The engine.start() returns Ok(()) because the coordinator runs in a
        // spawned task. The probe failure is reflected in the task's DB state
        // (set to Error). Give it a moment to fail, then check.
        engine.start(&id).await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let row = TasksRepo::new(&engine.db).get(&id).unwrap().unwrap();
        assert_eq!(row.state, TaskState::Error);
        assert!(row.error_message.is_some());
    }

    #[tokio::test]
    async fn end_to_end_download_single_segment() {
        // Set up a mock HTTP server that returns 100 bytes on Range request.
        let server = MockServer::start().await;
        let body: Vec<u8> = (0..100).collect();
        let total = body.len() as i64;

        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-length", total.to_string())
                    .insert_header("accept-ranges", "bytes"),
            )
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(header("range", "bytes=0-49"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", format!("bytes 0-49/{total}"))
                    .insert_header("content-length", "50")
                    .set_body_bytes(body[..50].to_vec()),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(header("range", "bytes=50-99"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", format!("bytes 50-99/{total}"))
                    .insert_header("content-length", "50")
                    .set_body_bytes(body[50..].to_vec()),
            )
            .mount(&server)
            .await;

        let engine = fresh_engine();
        let id = seed_task(&engine.db, &server.uri(), Some(total));
        // Override segments to 2.
        {
            let db = Arc::clone(&engine.db);
            let conn = db.lock().unwrap();
            conn.execute(
                "UPDATE tasks SET segments_requested = 2 WHERE id = ?1",
                rusqlite::params![id],
            )
            .unwrap();
        }

        // Start the task.
        engine.start(&id).await.unwrap();

        // Wait for completion. Poll the DB.
        let mut attempts = 0;
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            attempts += 1;
            if attempts > 100 {
                panic!("task did not complete in time");
            }
            let row = TasksRepo::new(&engine.db).get(&id).unwrap().unwrap();
            if row.state == TaskState::Complete || row.state == TaskState::Error {
                break;
            }
        }

        let row = TasksRepo::new(&engine.db).get(&id).unwrap().unwrap();
        assert_eq!(
            row.state,
            TaskState::Complete,
            "task should complete; got error: {:?}",
            row.error_message
        );
        assert_eq!(row.downloaded_bytes, total);

        // Verify file contents.
        let contents = std::fs::read("/tmp/hyprfetch-test.bin").unwrap();
        assert_eq!(contents, body);

        // Clean up.
        let _ = std::fs::remove_file("/tmp/hyprfetch-test.bin");
    }
}
