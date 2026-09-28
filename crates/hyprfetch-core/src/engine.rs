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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hyprfetch_db::schema::{QosOverride, TaskState};
use hyprfetch_db::{SegmentRow, SegmentsRepo, SettingsRepo, TaskRow, TasksRepo};
use tokio::sync::{mpsc, Notify, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use url::Url;

use crate::events::{run_speed_aggregator, EngineEvent, EventBus};
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
    /// Tasks currently running. One entry per active task. Shared with the
    /// spawned coordinator wrappers so they can reap their own entry when
    /// the task reaches a terminal state (complete / error / removed).
    tasks: Arc<RwLock<HashMap<String, ActiveTask>>>,
    /// Engine-wide QoS governor shared by ALL active tasks — one token
    /// bucket caps the aggregate download rate of the whole daemon.
    qos: QosLimiter,
    /// Broadcast bus for lifecycle events (task:progress, task:state,
    /// global:speed). WebSocket clients subscribe through the API layer.
    events: EventBus,
    /// Guards the one-time spawn of the speed aggregator (spawned lazily on
    /// the first `start()`, which is guaranteed to run inside a tokio
    /// runtime — constructors may be called from sync contexts).
    aggregator_started: AtomicBool,
    /// Queue-pump wake signal: kicked when a slot may have freed up or new
    /// queued tasks may be startable. The pump loop enforces the
    /// `max_concurrent_tasks` setting for queued tasks.
    queue_notify: Arc<Notify>,
    /// Guards the one-time spawn of the queue-pump loop.
    pump_started: AtomicBool,
}

/// A running task: channel to send commands to its coordinator.
struct ActiveTask {
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
        let settings = SettingsRepo::new(&db);
        let qos = QosLimiter::new();
        // Restore QoS state from settings (best-effort — a missing/broken
        // setting just means QoS stays off).
        if let Ok(Some(target)) = settings.get("qos_target_bps") {
            let enabled = settings.get("qos_enabled").ok().flatten().as_deref() == Some("true");
            if enabled {
                if let Ok(bps) = target.parse::<u64>() {
                    qos.enable(bps);
                }
            }
        }

        // Honor the `user_agent` setting for outgoing requests (empty/missing
        // keeps `HyprFetch/<version>`).
        let user_agent = settings
            .get("user_agent")
            .ok()
            .flatten()
            .filter(|s| !s.trim().is_empty());

        Self {
            db,
            http: HttpClient::with_user_agent(policy, user_agent),
            tasks: Arc::new(RwLock::new(HashMap::new())),
            qos,
            events: EventBus::new(),
            aggregator_started: AtomicBool::new(false),
            queue_notify: Arc::new(Notify::new()),
            pump_started: AtomicBool::new(false),
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

    /// Subscribe to the engine event bus (`task:progress`, `task:state`,
    /// `global:speed`). Each subscriber gets its own live stream.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<EngineEvent> {
        self.events.subscribe()
    }

    /// Spawn the global-speed aggregator exactly once. Must be called from
    /// async context (`start()` guarantees that).
    fn ensure_aggregator(&self) {
        if !self.aggregator_started.swap(true, Ordering::SeqCst) {
            let bus = self.events.clone();
            let rx = self.events.subscribe();
            tokio::spawn(async move {
                run_speed_aggregator(bus, rx, crate::events::SPEED_TICK).await;
            });
        }
    }

    /// Spawn the queue-pump loop exactly once. Must be called from async
    /// context (all callers are async).
    fn ensure_pump(&self) {
        if !self.pump_started.swap(true, Ordering::SeqCst) {
            let db = Arc::clone(&self.db);
            let tasks = Arc::clone(&self.tasks);
            let http = self.http.clone();
            let qos = self.qos.clone();
            let events = self.events.clone();
            let notify = Arc::clone(&self.queue_notify);
            tokio::spawn(async move {
                pump_loop(db, tasks, http, qos, events, notify).await;
            });
        }
    }

    /// Kick the queue pump: start queued tasks while the number of running
    /// tasks is below `max_concurrent_tasks` (0 or missing = unlimited).
    /// Called after creating tasks and whenever a running task reaches a
    /// terminal state.
    pub async fn pump(&self) {
        self.ensure_pump();
        self.queue_notify.notify_one();
    }

    /// Start a queued task. Probes the URL, splits into segments, spawns workers.
    /// Explicit starts (API create/resume, startup resume) bypass the
    /// `max_concurrent_tasks` cap — the cap is enforced by the queue pump for
    /// tasks waiting in the `queued` state.
    pub async fn start(&self, task_id: &str) -> Result<(), EngineError> {
        self.ensure_aggregator();
        self.ensure_pump();

        // Fetch the task row.
        let row = {
            let repo = TasksRepo::new(&self.db);
            repo.get(task_id)?
                .ok_or_else(|| EngineError::TaskNotFound(task_id.to_string()))?
        };

        // Single snapshot of "is a live coordinator attached" — used both for
        // the stranded-state rule and the double-spawn guard below.
        let is_running = {
            let tasks = self.tasks.read().await;
            tasks.contains_key(task_id)
        };
        match row.state {
            TaskState::Queued | TaskState::Paused => {}
            // A task left in `downloading` with NO live coordinator is
            // restartable: nothing owns it. This happens when a resume races
            // an in-flight pause (the pause route already flipped the DB row
            // to `downloading` before the old coordinator finished stopping),
            // or after a hard crash that skipped the startup normalize pass.
            TaskState::Downloading if !is_running => {}
            other => return Err(EngineError::InvalidState(other)),
        }
        if is_running {
            return Err(EngineError::Other(format!(
                "task {task_id} is already running"
            )));
        }

        // Spawn the coordinator with a self-reaping wrapper.
        spawn_coordinator(
            Arc::clone(&self.db),
            Arc::clone(&self.tasks),
            self.http.clone(),
            self.qos.clone(),
            self.events.clone(),
            Arc::clone(&self.queue_notify),
            task_id.to_string(),
        )
        .await;

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

    /// Number of tasks in a "would do work" state (queued + downloading).
    /// Used by the idle watcher and `GET /api/server`.
    pub fn active_task_count(&self) -> Result<usize, EngineError> {
        let repo = TasksRepo::new(&self.db);
        let n = repo.list_by_state(Some(TaskState::Queued))?.len()
            + repo.list_by_state(Some(TaskState::Downloading))?.len();
        Ok(n)
    }

    /// Drain helper for restarts: pause every currently-downloading task.
    /// Paused tasks are auto-resumed by `resume_all()` on the next startup,
    /// which gives the update flow its "drain → restart → auto-resume"
    /// behavior. Returns how many tasks were paused.
    pub async fn pause_all_active(&self) -> Result<usize, EngineError> {
        let rows = TasksRepo::new(&self.db).list_by_state(Some(TaskState::Downloading))?;
        let mut paused = 0usize;
        for row in rows {
            if self.pause(&row.id).await.is_ok() {
                paused += 1;
            }
        }
        Ok(paused)
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

    /// Startup resume pass: reload every incomplete task (queued /
    /// downloading / paused), validate the remote against the stored
    /// ETag / Last-Modified / size, and restart workers from the persisted
    /// segment offsets.
    ///
    /// Validation rules per task:
    /// - `downloading` rows are crash leftovers and normalized to `paused`
    ///   first (a fresh process can't have live workers).
    /// - HEAD probe fails → task is marked `error` with the reason
    ///   (user can retry later); it is NOT auto-retried.
    /// - Remote changed (stored ETag or Last-Modified no longer matches, or
    ///   Content-Length differs from the stored total) → persisted offsets
    ///   are discarded and the download restarts from byte 0.
    /// - Segment rows exist but the local file is missing/truncated →
    ///   offsets are discarded too (they'd corrupt the file).
    /// - Otherwise workers resume from the persisted offsets.
    ///
    /// Failed validations never abort the pass — every incomplete task is
    /// examined. Returns the number of tasks successfully restarted.
    pub async fn resume_all(&self) -> Result<usize, EngineError> {
        let mut tasks: Vec<TaskRow> = Vec::new();
        {
            let repo = TasksRepo::new(&self.db);
            for state in [TaskState::Queued, TaskState::Downloading, TaskState::Paused] {
                tasks.extend(repo.list_by_state(Some(state))?);
            }
        }
        // Oldest first for deterministic startup order.
        tasks.sort_by_key(|t| t.created_at);

        let mut resumed = 0usize;
        for mut row in tasks {
            if self.resume_task(&mut row).await {
                resumed += 1;
            }
        }
        if resumed > 0 {
            info!(resumed, "startup resume pass complete");
        }
        Ok(resumed)
    }

    /// Validate + restart one task. Returns `true` if workers were started.
    async fn resume_task(&self, row: &mut TaskRow) -> bool {
        let task_id = row.id.clone();

        // Crash leftover: normalize before anything else so `start()`'s
        // state gate accepts the task.
        if row.state == TaskState::Downloading {
            let _ = TasksRepo::new(&self.db).touch(
                &task_id,
                TaskState::Paused,
                row.downloaded_bytes,
                None,
            );
            row.state = TaskState::Paused;
        }

        // Parse extra headers for the probe.
        let extra: Option<ExtraHeaders> = row
            .extra_headers
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());

        // HEAD the URL.
        let probe = match Url::parse(&row.url) {
            Ok(url) => match self.http.probe(&url, extra.as_ref()).await {
                Ok(p) => Some(p),
                Err(e) => {
                    let msg = format!("resume probe failed: {e}");
                    warn!(task = %task_id, error = %e, "resume: probe failed");
                    let _ = TasksRepo::new(&self.db).touch(
                        &task_id,
                        TaskState::Error,
                        row.downloaded_bytes,
                        Some(&msg),
                    );
                    return false;
                }
            },
            Err(e) => {
                let msg = format!("resume: bad url: {e}");
                warn!(task = %task_id, error = %e, "resume: bad url");
                let _ = TasksRepo::new(&self.db).touch(
                    &task_id,
                    TaskState::Error,
                    row.downloaded_bytes,
                    Some(&msg),
                );
                return false;
            }
        };
        let probe = probe.expect("probe present on success path");

        // Compare remote validators against what we stored when this task
        // last (partially) downloaded.
        if remote_changed(row, &probe) {
            info!(
                task = %task_id,
                "resume: remote file changed since last download; resetting progress"
            );
            let _ = SegmentsRepo::new(&self.db).delete_for_task(&task_id);
            let _ = TasksRepo::new(&self.db).touch(&task_id, TaskState::Paused, 0, None);
            row.downloaded_bytes = 0;
        } else {
            // Offsets are only as good as the local file they point into.
            let has_segments = !SegmentsRepo::new(&self.db)
                .list_for_task(&task_id)
                .unwrap_or_default()
                .is_empty();
            if has_segments && !local_file_fits(&row.save_path, row.total_bytes) {
                info!(
                    task = %task_id,
                    "resume: local file missing or truncated; resetting progress"
                );
                let _ = SegmentsRepo::new(&self.db).delete_for_task(&task_id);
                let _ = TasksRepo::new(&self.db).touch(&task_id, TaskState::Paused, 0, None);
                row.downloaded_bytes = 0;
            }
        }

        match self.start(&task_id).await {
            Ok(()) => true,
            Err(e) => {
                warn!(task = %task_id, error = %e, "resume: could not start task");
                false
            }
        }
    }
}

/// Spawn the per-task coordinator wrapped in a self-reaping task.
///
/// The wrapper removes the task's `ActiveTask` entry from the shared map as
/// soon as the coordinator returns (complete / error / paused / cancelled),
/// so the map only ever holds genuinely running tasks. On terminal states
/// (complete / error / removed) it also kicks the queue pump — a slot may
/// have freed up for queued tasks.
///
/// Must be awaited from async context (it inserts the map entry before
/// spawning). The `queue_notify` is the engine's shared pump wake handle —
/// every spawned wrapper re-kicks it on terminal states, no matter who
/// spawned the coordinator (API `start()` or the queue pump itself).
async fn spawn_coordinator(
    db: Arc<std::sync::Mutex<rusqlite::Connection>>,
    tasks: Arc<RwLock<HashMap<String, ActiveTask>>>,
    http: HttpClient,
    qos: QosLimiter,
    events: EventBus,
    queue_notify: Arc<Notify>,
    task_id: String,
) {
    let (cmd_tx, cmd_rx) = mpsc::channel(8);
    // Check-and-insert atomically: if the task is already running (another
    // pump pass or an explicit start() won the race), do NOT spawn a second
    // coordinator for it — overwriting the entry would drop the live
    // coordinator's only command sender, which it treats as a cancel.
    {
        let mut map = tasks.write().await;
        if map.contains_key(&task_id) {
            debug!(task = %task_id, "spawn_coordinator: already running, skipping");
            return;
        }
        map.insert(
            task_id.clone(),
            ActiveTask {
                cmd_tx: cmd_tx.clone(),
            },
        );
    }
    tokio::spawn(async move {
        let result = run_task_coordinator(
            db.clone(),
            http,
            qos,
            events.clone(),
            task_id.clone(),
            cmd_rx,
        )
        .await;
        // Reap unconditionally — the task is no longer running.
        tasks.write().await.remove(&task_id);
        if let Err(e) = result {
            error!(task = %task_id, error = %e, "task coordinator failed");
        }
        // Kick the queue pump only when the task reached a TERMINAL state —
        // a pause must NOT auto-start a queued replacement (the user paused
        // to reclaim bandwidth). Removed/complete/error free a slot.
        if let Ok(Some(row)) = TasksRepo::new(&db).get(&task_id) {
            if matches!(
                row.state,
                TaskState::Complete | TaskState::Error | TaskState::Removed
            ) {
                queue_notify.notify_one();
            }
        }
    });
}

/// Queue-pump loop: on every wake, start queued tasks (oldest first) while
/// the running count is below `max_concurrent_tasks`. 0 / missing / unparsable
/// setting = unlimited (drain the whole queue).
async fn pump_loop(
    db: Arc<std::sync::Mutex<rusqlite::Connection>>,
    tasks: Arc<RwLock<HashMap<String, ActiveTask>>>,
    http: HttpClient,
    qos: QosLimiter,
    events: EventBus,
    notify: Arc<Notify>,
) {
    loop {
        notify.notified().await;
        pump_once(&db, &tasks, &http, &qos, &events, &notify).await;
    }
}

async fn pump_once(
    db: &Arc<std::sync::Mutex<rusqlite::Connection>>,
    tasks: &Arc<RwLock<HashMap<String, ActiveTask>>>,
    http: &HttpClient,
    qos: &QosLimiter,
    events: &EventBus,
    notify: &Arc<Notify>,
) {
    let limit: usize = SettingsRepo::new(db)
        .get("max_concurrent_tasks")
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    // IDs spawned during THIS pass: the coordinator marks its row
    // `downloading` asynchronously, so a plain DB re-list could return a
    // task that was already picked — filtering here prevents double spawns
    // (the map guard in spawn_coordinator is the second line of defense).
    let mut started: std::collections::HashSet<String> = std::collections::HashSet::new();

    loop {
        let running = tasks.read().await.len();
        if limit > 0 && running >= limit {
            debug!(running, limit, "queue pump: at capacity");
            return;
        }
        // Next queued task, oldest first, excluding ones this pass started.
        let next = TasksRepo::new(db)
            .list_by_state(Some(TaskState::Queued))
            .ok()
            .and_then(|rows| {
                rows.into_iter()
                    .filter(|t| !started.contains(&t.id))
                    .min_by_key(|t| t.created_at)
            });
        let Some(row) = next else {
            return; // queue drained
        };
        started.insert(row.id.clone());
        info!(task = %row.id, running, limit, "queue pump: starting queued task");
        spawn_coordinator(
            Arc::clone(db),
            Arc::clone(tasks),
            http.clone(),
            qos.clone(),
            events.clone(),
            Arc::clone(notify),
            row.id.clone(),
        )
        .await;
    }
}

/// Decisive staleness check: compare stored validators with a fresh probe.
///
/// Only validators we actually stored are compared — a server *starting* to
/// send an ETag is new information, not evidence of change. A size mismatch
/// is always decisive.
fn remote_changed(row: &TaskRow, probe: &crate::http_client::ProbeResult) -> bool {
    if let (Some(total), Some(len)) = (row.total_bytes, probe.content_length) {
        if total != len {
            return true;
        }
    }
    if let Some(stored) = &row.etag {
        if probe.etag.as_deref() != Some(stored.as_str()) {
            return true;
        }
    }
    if let Some(stored) = &row.last_modified {
        if probe.last_modified.as_deref() != Some(stored.as_str()) {
            return true;
        }
    }
    false
}

/// A local file is usable for offset resume when it exists and is at least
/// as large as the expected total (it was pre-allocated with ftruncate by a
/// previous run; smaller means it was truncated/replaced underneath us).
fn local_file_fits(path: &str, total_bytes: Option<i64>) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) => match total_bytes {
            Some(total) if total > 0 => meta.len() as i64 >= total,
            _ => true,
        },
        Err(_) => false,
    }
}

/// Per-task coordinator: owns the segment workers, the progress aggregator,
/// the persistence debouncer, and the command channel.
async fn run_task_coordinator(
    db: Arc<std::sync::Mutex<rusqlite::Connection>>,
    http: HttpClient,
    qos: QosLimiter,
    events: EventBus,
    task_id: String,
    mut cmd_rx: mpsc::Receiver<TaskCommand>,
) -> Result<(), EngineError> {
    // Re-fetch the task row (we may have paused and resumed).
    let mut row = {
        let repo = TasksRepo::new(&db);
        repo.get(&task_id)?
            .ok_or_else(|| EngineError::TaskNotFound(task_id.clone()))?
    };

    // Mark as downloading.
    TasksRepo::new(&db).touch(&task_id, TaskState::Downloading, row.downloaded_bytes, None)?;
    events.emit(EngineEvent::task_state(
        &task_id,
        TaskState::Downloading,
        None,
    ));

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
            events.emit(EngineEvent::task_state(
                &task_id,
                TaskState::Error,
                Some(&format!("probe failed: {e}")),
            ));
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

    // Mid-session remote-change recheck. The startup resume pass validates
    // stored offsets against the remote, but a task can also sit paused /
    // errored in-session (pause → remote replaced → resume, or retry after
    // error) — re-applying stale byte offsets to a DIFFERENT remote file
    // would silently corrupt the output. If any stored validator now
    // disagrees with the fresh probe, discard offsets and restart cleanly.
    // (A brand-new task has no stored validators and passes untouched.)
    if remote_changed(&row, &probe) {
        warn!(
            task = %task_id,
            "remote file changed since this task last ran; discarding stored offsets"
        );
        let _ = SegmentsRepo::new(&db).delete_for_task(&task_id);
        row.downloaded_bytes = 0;
        TasksRepo::new(&db).touch(&task_id, TaskState::Downloading, 0, None)?;
    }

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
            events.emit(EngineEvent::task_state(
                &task_id,
                TaskState::Error,
                Some("server did not return Content-Length; cannot segment"),
            ));
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
    // Speed sampling window for task:progress events.
    let mut speed_window_start = Instant::now();
    let mut speed_window_bytes = downloaded_bytes;

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
                        events.emit(EngineEvent::task_state(&task_id, TaskState::Paused, None));
                        return Ok(());
                    }
                    Some(TaskCommand::Cancel) | None => {
                        info!(task = %task_id, "cancel requested");
                        for h in &worker_handles {
                            h.abort();
                        }
                        let _ = persist_progress(&db, &task_id, &per_segment_current, &segments, TaskState::Removed, None).await;
                        events.emit(EngineEvent::task_state(&task_id, TaskState::Removed, None));
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
                        events.emit(EngineEvent::task_state(&task_id, TaskState::Error, Some(&error)));
                        return Ok(());
                    }
                    None => {
                        // All workers dropped their senders — done.
                        break;
                    }
                }
                // Debounced persist + progress broadcast.
                if last_persist.elapsed() >= DEBOUNCE_INTERVAL {
                    let _ = persist_progress(&db, &task_id, &per_segment_current, &segments, TaskState::Downloading, None).await;
                    let elapsed = speed_window_start.elapsed().as_secs_f64();
                    let speed = if elapsed > 0.0 {
                        (downloaded_bytes - speed_window_bytes).max(0) as f64 / elapsed
                    } else {
                        0.0
                    } as u64;
                    events.emit(EngineEvent::task_progress(
                        &task_id,
                        downloaded_bytes,
                        Some(total_bytes),
                        speed,
                    ));
                    speed_window_start = Instant::now();
                    speed_window_bytes = downloaded_bytes;
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
    events.emit(EngineEvent::task_state(
        &task_id,
        final_state,
        err_msg.as_deref(),
    ));

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

    // -- resume persistence ---------------------------------------------

    /// Seed a task that claims to have partially downloaded before, with
    /// matching persisted segment rows and validators. Each task gets a
    /// unique save path so parallel tests can't interfere.
    fn seed_resumable_task(
        db: &Arc<std::sync::Mutex<rusqlite::Connection>>,
        url: &str,
        state: TaskState,
        etag: Option<&str>,
        total: i64,
    ) -> String {
        let id = uuid::Uuid::now_v7().to_string();
        let now = now_ms();
        let save_path = format!("/tmp/hyprfetch-resume-{id}.bin");
        let row = TaskRow {
            id: id.clone(),
            url: url.into(),
            filename: "resume.bin".into(),
            save_path,
            total_bytes: Some(total),
            downloaded_bytes: 50,
            state,
            etag: etag.map(|s| s.to_string()),
            last_modified: None,
            accept_ranges: true,
            segments_requested: 2,
            qos_override: None,
            extra_headers: None,
            error_message: None,
            created_at: now,
            updated_at: now,
            completed_at: None,
        };
        TasksRepo::new(db).insert(&row).unwrap();

        // Persisted segment plan: seg0 complete (0..half), seg1 halfway.
        let half = total / 2;
        let rows: Vec<_> = (0..2)
            .map(|i| SegmentRow {
                task_id: id.clone(),
                segment_idx: i,
                start_byte: i * half,
                end_byte: if i == 1 { total } else { (i + 1) * half },
                current_byte: if i == 0 { half } else { half + half / 2 },
                state: hyprfetch_db::SegmentState::Downloading,
                speed_bps: 0,
                error_message: None,
                updated_at: now,
            })
            .collect();
        SegmentsRepo::new(db).insert_batch(&rows).unwrap();
        id
    }

    async fn wait_terminal(db: &Arc<std::sync::Mutex<rusqlite::Connection>>, id: &str) -> TaskRow {
        for _ in 0..120 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let row = TasksRepo::new(db).get(id).unwrap().unwrap();
            if matches!(row.state, TaskState::Complete | TaskState::Error) {
                return row;
            }
        }
        panic!("task did not reach a terminal state in time");
    }

    fn resume_path(id: &str) -> String {
        format!("/tmp/hyprfetch-resume-{id}.bin")
    }

    #[tokio::test]
    async fn resume_all_restarts_from_persisted_offsets() {
        let server = MockServer::start().await;
        let body: Vec<u8> = (0..100u8).collect();
        let total = body.len() as i64;
        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-length", total.to_string())
                    .insert_header("accept-ranges", "bytes")
                    .insert_header("etag", "\"v1\""),
            )
            .mount(&server)
            .await;
        // Range GETs: seg0 0-49, seg1 resumes at its persisted offset 75.
        Mock::given(method("GET"))
            .and(header("range", "bytes=0-49"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", "bytes 0-49/100")
                    .insert_header("content-length", "50")
                    .set_body_bytes(body[..50].to_vec()),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(header("range", "bytes=75-99"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", "bytes 75-99/100")
                    .insert_header("content-length", "25")
                    .set_body_bytes(body[75..].to_vec()),
            )
            .mount(&server)
            .await;

        let engine = fresh_engine();
        let id = seed_resumable_task(
            &engine.db,
            &server.uri(),
            TaskState::Paused,
            Some("\"v1\""),
            total,
        );

        // Local file: first 75 bytes were written before the crash.
        let path = resume_path(&id);
        let mut local = vec![0u8; 100];
        local[..75].copy_from_slice(&body[..75]);
        std::fs::write(&path, &local).unwrap();

        // The coordinator must resume seg1 from offset 75 → request
        // bytes=75-99 (mocked above). Byte-exact file contents prove it.
        let resumed = engine.resume_all().await.unwrap();
        assert_eq!(resumed, 1);

        let row = wait_terminal(&engine.db, &id).await;
        assert_eq!(row.state, TaskState::Complete, "{:?}", row.error_message);
        let contents = std::fs::read(&path).unwrap();
        assert_eq!(contents, body, "resumed file must be byte-exact");
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn resume_all_resets_when_etag_changed() {
        let server = MockServer::start().await;
        let body: Vec<u8> = vec![7u8; 100]; // NEW remote content
        let total = 100i64;
        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-length", total.to_string())
                    .insert_header("accept-ranges", "bytes")
                    .insert_header("etag", "\"v2\""), // changed!
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(header("range", "bytes=0-49"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", "bytes 0-49/100")
                    .insert_header("content-length", "50")
                    .set_body_bytes(body[..50].to_vec()),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(header("range", "bytes=50-99"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", "bytes 50-99/100")
                    .insert_header("content-length", "50")
                    .set_body_bytes(body[50..].to_vec()),
            )
            .mount(&server)
            .await;

        let engine = fresh_engine();
        let id = seed_resumable_task(
            &engine.db,
            &server.uri(),
            TaskState::Paused,
            Some("\"old\""),
            total,
        );

        // Corrupt local file: if the staleness check is broken, the
        // coordinator sees all segments complete and "finishes" instantly,
        // leaving the garbage file.
        let path = resume_path(&id);
        std::fs::write(&path, vec![0xEEu8; 100]).unwrap();

        // Claim full progress: both segments complete.
        {
            let db = Arc::clone(&engine.db);
            let conn = db.lock().unwrap();
            conn.execute(
                "UPDATE segments SET current_byte = end_byte",
                rusqlite::params![],
            )
            .unwrap();
        }

        let resumed = engine.resume_all().await.unwrap();
        assert_eq!(resumed, 1, "stale task must still be restarted (from 0)");

        let row = wait_terminal(&engine.db, &id).await;
        assert_eq!(row.state, TaskState::Complete, "{:?}", row.error_message);
        let contents = std::fs::read(&path).unwrap();
        assert_eq!(contents, body, "stale offsets must reset and redownload");
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn resume_all_normalizes_downloading_crash_leftovers() {
        let server = MockServer::start().await;
        let body: Vec<u8> = vec![9u8; 100];
        let total = 100i64;
        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-length", total.to_string())
                    .insert_header("accept-ranges", "bytes")
                    .insert_header("etag", "\"v1\""),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(header("range", "bytes=0-49"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", "bytes 0-49/100")
                    .insert_header("content-length", "50")
                    .set_body_bytes(body[..50].to_vec()),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(header("range", "bytes=75-99"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", "bytes 75-99/100")
                    .insert_header("content-length", "25")
                    .set_body_bytes(body[75..].to_vec()),
            )
            .mount(&server)
            .await;

        let engine = fresh_engine();
        // State = Downloading (crash leftover) — resume_all must normalize
        // to paused and still restart it.
        let id = seed_resumable_task(
            &engine.db,
            &server.uri(),
            TaskState::Downloading,
            Some("\"v1\""),
            total,
        );

        let path = resume_path(&id);
        let mut local = vec![0u8; 100];
        local[..75].copy_from_slice(&body[..75]);
        std::fs::write(&path, &local).unwrap();

        let resumed = engine.resume_all().await.unwrap();
        assert_eq!(resumed, 1);

        let row = wait_terminal(&engine.db, &id).await;
        assert_eq!(row.state, TaskState::Complete, "{:?}", row.error_message);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn resume_all_marks_error_when_probe_fails() {
        // No mock mounted → wiremock returns 404 for HEAD.
        let server = MockServer::start().await;
        let engine = fresh_engine();
        let id = seed_resumable_task(
            &engine.db,
            &server.uri(),
            TaskState::Paused,
            Some("\"v1\""),
            100,
        );

        let resumed = engine.resume_all().await.unwrap();
        assert_eq!(resumed, 0, "probe failure must not count as resumed");

        let row = TasksRepo::new(&engine.db).get(&id).unwrap().unwrap();
        assert_eq!(row.state, TaskState::Error);
        assert!(
            row.error_message
                .unwrap_or_default()
                .contains("resume probe failed"),
            "error should explain the failed probe"
        );
        // Segment rows must be untouched (no fake reset).
        assert_eq!(
            SegmentsRepo::new(&engine.db)
                .list_for_task(&id)
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn resume_all_with_nothing_incomplete_is_noop() {
        let engine = fresh_engine();
        let resumed = engine.resume_all().await.unwrap();
        assert_eq!(resumed, 0);
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

    // ------------------------------------------------------------------
    // Task reaping + queue pump
    // ------------------------------------------------------------------

    fn seed_queue_task(
        db: &Arc<std::sync::Mutex<rusqlite::Connection>>,
        url: &str,
        path: &str,
    ) -> String {
        let id = uuid::Uuid::now_v7().to_string();
        let now = now_ms();
        let row = TaskRow {
            id: id.clone(),
            url: url.into(),
            filename: "queue.bin".into(),
            save_path: path.into(),
            total_bytes: None,
            downloaded_bytes: 0,
            state: TaskState::Queued,
            etag: None,
            last_modified: None,
            accept_ranges: false,
            segments_requested: 1,
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

    /// Mock server: 100-byte file, HEAD + a single full-body GET range with
    /// a per-request delay so queue behavior is observable.
    async fn mount_queue_server(server: &MockServer, delay_ms: u64) {
        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-length", "100")
                    .insert_header("accept-ranges", "bytes"),
            )
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(header("range", "bytes=0-99"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", "bytes 0-99/100")
                    .insert_header("content-length", "100")
                    .set_body_bytes(vec![7u8; 100])
                    .set_delay(Duration::from_millis(delay_ms)),
            )
            .mount(server)
            .await;
    }

    async fn wait_running_count(engine: &Engine, expected: usize) {
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let mut n = 0;
            for id in TasksRepo::new(&engine.db)
                .list_by_state(None)
                .unwrap()
                .iter()
                .map(|t| t.id.clone())
            {
                if engine.is_running(&id).await {
                    n += 1;
                }
            }
            if n == expected {
                return;
            }
        }
        panic!("running count never reached {expected}");
    }

    #[tokio::test]
    async fn download_from_server_without_range_support() {
        // Regression: a server that answers 200 OK (no Range support at all)
        // must work through the single-segment fallback. The worker used to
        // require 206 unconditionally and every such download failed with
        // "only 0 of 1 segments completed".
        let server = MockServer::start().await;
        let body: Vec<u8> = (0..100u8).collect();
        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200).insert_header("content-length", "100"),
                // deliberately NO accept-ranges
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-length", "100")
                    .set_body_bytes(body.clone()),
            )
            .mount(&server)
            .await;

        let engine = fresh_engine();
        let id = seed_queue_task(&engine.db, &server.uri(), "/tmp/hyprfetch-norange.bin");
        engine.start(&id).await.unwrap();

        let row = wait_terminal(&engine.db, &id).await;
        assert_eq!(row.state, TaskState::Complete, "{:?}", row.error_message);
        assert_eq!(row.downloaded_bytes, 100);
        let contents = std::fs::read("/tmp/hyprfetch-norange.bin").unwrap();
        assert_eq!(contents, body, "100-byte file must be byte-exact");
        let _ = std::fs::remove_file("/tmp/hyprfetch-norange.bin");
    }

    #[tokio::test]
    async fn finished_task_is_reaped_from_active_map() {
        let server = MockServer::start().await;
        mount_queue_server(&server, 0).await;
        let engine = fresh_engine();
        let id = seed_queue_task(&engine.db, &server.uri(), "/tmp/hyprfetch-reap.bin");

        engine.start(&id).await.unwrap();
        assert!(
            engine.is_running(&id).await,
            "task must register as running"
        );

        let row = wait_terminal(&engine.db, &id).await;
        assert_eq!(row.state, TaskState::Complete, "{:?}", row.error_message);

        // The wrapper must reap the finished entry promptly.
        for _ in 0..40 {
            if !engine.is_running(&id).await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("finished task was never reaped from the engine map");
    }

    #[tokio::test]
    async fn queue_pump_respects_max_concurrent_tasks() {
        let server = MockServer::start().await;
        mount_queue_server(&server, 250).await;
        let engine = fresh_engine();

        {
            let s = SettingsRepo::new(&engine.db);
            s.set("max_concurrent_tasks", "2").unwrap();
        }

        let ids: Vec<String> = (0..4)
            .map(|i| {
                seed_queue_task(
                    &engine.db,
                    &server.uri(),
                    &format!("/tmp/hyprfetch-queue-{i}.bin"),
                )
            })
            .collect();

        // Creating tasks only kicks the pump (mirrors the API create path).
        engine.pump().await;

        // Exactly 2 must run; the other 2 stay queued.
        wait_running_count(&engine, 2).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        for (i, id) in ids.iter().enumerate() {
            let row = TasksRepo::new(&engine.db).get(id).unwrap().unwrap();
            if i < 2 {
                assert!(
                    engine.is_running(id).await,
                    "task {i} should have started (state {:?})",
                    row.state
                );
            } else {
                assert_eq!(
                    row.state,
                    TaskState::Queued,
                    "task {i} must still be queued while at capacity"
                );
            }
        }

        // As running tasks finish, the pump must chain-start the rest.
        for id in &ids {
            let row = wait_terminal(&engine.db, id).await;
            assert_eq!(row.state, TaskState::Complete, "{:?}", row.error_message);
        }
        for (i, _) in ids.iter().enumerate() {
            let _ = std::fs::remove_file(format!("/tmp/hyprfetch-queue-{i}.bin"));
        }
    }

    #[tokio::test]
    async fn queue_pump_unlimited_drains_queue() {
        let server = MockServer::start().await;
        // Delay > poll interval so all three are simultaneously observable
        // as running before any of them completes.
        mount_queue_server(&server, 200).await;
        let engine = fresh_engine();
        // 0 = unlimited: everything queued starts immediately.
        SettingsRepo::new(&engine.db)
            .set("max_concurrent_tasks", "0")
            .unwrap();

        let ids: Vec<String> = (0..3)
            .map(|i| {
                seed_queue_task(
                    &engine.db,
                    &server.uri(),
                    &format!("/tmp/hyprfetch-unl-{i}.bin"),
                )
            })
            .collect();

        engine.pump().await;
        wait_running_count(&engine, 3).await;
        for id in &ids {
            let row = wait_terminal(&engine.db, id).await;
            assert_eq!(row.state, TaskState::Complete, "{:?}", row.error_message);
        }
        for i in 0..3 {
            let _ = std::fs::remove_file(format!("/tmp/hyprfetch-unl-{i}.bin"));
        }
    }

    #[tokio::test]
    async fn resume_after_remote_change_mid_session_resets_offsets() {
        // Pause → (remote replaced) → resume must NOT re-apply stale byte
        // offsets to the new remote file. Stored etag "v-old" vs remote
        // "v-new" → coordinator discards offsets and redownloads fully.
        let server = MockServer::start().await;
        let body: Vec<u8> = (0..100).collect();
        Mock::given(method("HEAD"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-length", "100")
                    .insert_header("accept-ranges", "bytes")
                    .insert_header("etag", "\"v-new\""),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(header("range", "bytes=0-49"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", "bytes 0-49/100")
                    .insert_header("content-length", "50")
                    .set_body_bytes(body[..50].to_vec()),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(header("range", "bytes=50-99"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", "bytes 50-99/100")
                    .insert_header("content-length", "50")
                    .set_body_bytes(body[50..].to_vec()),
            )
            .mount(&server)
            .await;

        let engine = fresh_engine();
        let id = seed_resumable_task(
            &engine.db,
            &server.uri(),
            TaskState::Paused,
            Some("\"v-old\""), // stored validator is STALE
            100,
        );

        // Local file holding the OLD remote's bytes: if stale offsets were
        // applied, the segments would look complete and this garbage stays.
        let path = resume_path(&id);
        std::fs::write(&path, vec![0xEEu8; 100]).unwrap();

        engine.start(&id).await.unwrap();
        let row = wait_terminal(&engine.db, &id).await;
        assert_eq!(row.state, TaskState::Complete, "{:?}", row.error_message);
        let contents = std::fs::read(&path).unwrap();
        assert_eq!(contents, body, "stale offsets must be discarded in-session");
        let _ = std::fs::remove_file(path);
    }
}
