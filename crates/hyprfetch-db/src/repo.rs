//! Repository pattern for `tasks`, `segments`, `settings`, `events`.
//!
//! Each repo takes a `&Arc<Mutex<Connection>>` and exposes typed methods.
//! All writes use explicit transactions so partial updates can't happen.

use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};

use crate::schema::{QosOverride, SegmentState, TaskState};

/// Convenience: acquire the connection lock, run a closure, release.
fn with_conn<T>(
    db: &Arc<Mutex<Connection>>,
    f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    let conn = db.lock().expect("db mutex poisoned");
    f(&conn)
}

/// A timestamp in Unix milliseconds.
pub type Ts = i64;

/// One row of the `tasks` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
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
    pub extra_headers: Option<String>, // JSON
    pub error_message: Option<String>,
    pub created_at: Ts,
    pub updated_at: Ts,
    pub completed_at: Option<Ts>,
}

/// One row of the `segments` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentRow {
    pub task_id: String,
    pub segment_idx: i64,
    pub start_byte: i64,
    pub end_byte: i64,
    pub current_byte: i64,
    pub state: SegmentState,
    pub speed_bps: i64,
    pub error_message: Option<String>,
    pub updated_at: Ts,
}

/// One row of the `events` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRow {
    pub id: i64,
    pub task_id: Option<String>,
    pub kind: String,
    pub payload: String, // JSON
    pub ts: Ts,
}

/// Repository for the `tasks` table.
pub struct TasksRepo<'a> {
    db: &'a Arc<Mutex<Connection>>,
}

impl<'a> TasksRepo<'a> {
    pub fn new(db: &'a Arc<Mutex<Connection>>) -> Self {
        Self { db }
    }

    /// Insert a new task row. Returns the inserted row's id.
    pub fn insert(&self, row: &TaskRow) -> rusqlite::Result<()> {
        with_conn(self.db, |c| {
            c.execute(
                "INSERT INTO tasks (
                    id, url, filename, save_path,
                    total_bytes, downloaded_bytes, state,
                    etag, last_modified, accept_ranges,
                    segments_requested, qos_override, extra_headers,
                    error_message, created_at, updated_at, completed_at
                ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
                params![
                    row.id,
                    row.url,
                    row.filename,
                    row.save_path,
                    row.total_bytes,
                    row.downloaded_bytes,
                    row.state.as_str(),
                    row.etag,
                    row.last_modified,
                    row.accept_ranges as i64,
                    row.segments_requested,
                    row.qos_override.map(|o| o.as_str()),
                    row.extra_headers,
                    row.error_message,
                    row.created_at,
                    row.updated_at,
                    row.completed_at,
                ],
            )
        })?;
        Ok(())
    }

    /// Fetch a single task by id. `Ok(None)` if not found.
    pub fn get(&self, id: &str) -> rusqlite::Result<Option<TaskRow>> {
        with_conn(self.db, |c| {
            let mut stmt = c.prepare("SELECT * FROM tasks WHERE id = ?1")?;
            let mut rows = stmt.query(params![id])?;
            match rows.next()? {
                Some(r) => Ok(Some(row_to_task(r)?)),
                None => Ok(None),
            }
        })
    }

    /// List tasks by state.
    pub fn list_by_state(&self, state: Option<TaskState>) -> rusqlite::Result<Vec<TaskRow>> {
        with_conn(self.db, |c| {
            let sql = if state.is_some() {
                "SELECT * FROM tasks WHERE state = ?1 ORDER BY created_at DESC"
            } else {
                "SELECT * FROM tasks ORDER BY created_at DESC"
            };
            let mut stmt = c.prepare(sql)?;
            let rows = if let Some(s) = state {
                stmt.query_map(params![s.as_str()], row_to_task)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            } else {
                stmt.query_map([], row_to_task)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            Ok(rows)
        })
    }

    /// Atomically update a task's state, downloaded_bytes, and updated_at.
    pub fn touch(
        &self,
        id: &str,
        state: TaskState,
        downloaded_bytes: i64,
        error: Option<&str>,
    ) -> rusqlite::Result<()> {
        let now = now_ms();
        with_conn(self.db, |c| {
            c.execute(
                "UPDATE tasks
                 SET state = ?1, downloaded_bytes = ?2, error_message = ?3,
                     updated_at = ?4, completed_at = CASE WHEN ?1 = 'complete' THEN ?4 ELSE completed_at END
                 WHERE id = ?5",
                params![state.as_str(), downloaded_bytes, error, now, id],
            )
        })?;
        Ok(())
    }

    /// Persist cache validators (etag / last_modified) and accept_ranges flag.
    pub fn update_cache_validators(
        &self,
        id: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
        accept_ranges: bool,
        total_bytes: Option<i64>,
    ) -> rusqlite::Result<()> {
        let now = now_ms();
        with_conn(self.db, |c| {
            c.execute(
                "UPDATE tasks
                 SET etag = ?1, last_modified = ?2, accept_ranges = ?3,
                     total_bytes = ?4, updated_at = ?5
                 WHERE id = ?6",
                params![
                    etag,
                    last_modified,
                    accept_ranges as i64,
                    total_bytes,
                    now,
                    id
                ],
            )
        })?;
        Ok(())
    }

    /// Remove a task row (cascades to segments via FK).
    pub fn delete(&self, id: &str) -> rusqlite::Result<()> {
        with_conn(self.db, |c| {
            c.execute("DELETE FROM tasks WHERE id = ?1", params![id])
        })?;
        Ok(())
    }
}

/// Repository for the `segments` table.
pub struct SegmentsRepo<'a> {
    db: &'a Arc<Mutex<Connection>>,
}

impl<'a> SegmentsRepo<'a> {
    pub fn new(db: &'a Arc<Mutex<Connection>>) -> Self {
        Self { db }
    }

    /// Insert all segments for a task in a single transaction.
    pub fn insert_batch(&self, segments: &[SegmentRow]) -> rusqlite::Result<()> {
        with_conn(self.db, |c| {
            let tx = c.unchecked_transaction()?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO segments
                     (task_id, segment_idx, start_byte, end_byte, current_byte,
                      state, speed_bps, error_message, updated_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                )?;
                for s in segments {
                    stmt.execute(params![
                        s.task_id,
                        s.segment_idx,
                        s.start_byte,
                        s.end_byte,
                        s.current_byte,
                        s.state.as_str(),
                        s.speed_bps,
                        s.error_message,
                        s.updated_at,
                    ])?;
                }
            }
            tx.commit()
        })?;
        Ok(())
    }

    /// Fetch all segments for a task, ordered by `segment_idx`.
    pub fn list_for_task(&self, task_id: &str) -> rusqlite::Result<Vec<SegmentRow>> {
        with_conn(self.db, |c| {
            let mut stmt =
                c.prepare("SELECT * FROM segments WHERE task_id = ?1 ORDER BY segment_idx ASC")?;
            let rows = stmt
                .query_map(params![task_id], row_to_segment)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Update a single segment's progress (called from segment workers, debounced).
    pub fn touch(
        &self,
        task_id: &str,
        segment_idx: i64,
        current_byte: i64,
        state: SegmentState,
        speed_bps: i64,
    ) -> rusqlite::Result<()> {
        let now = now_ms();
        with_conn(self.db, |c| {
            c.execute(
                "UPDATE segments
                 SET current_byte = ?1, state = ?2, speed_bps = ?3, updated_at = ?4
                 WHERE task_id = ?5 AND segment_idx = ?6",
                params![
                    current_byte,
                    state.as_str(),
                    speed_bps,
                    now,
                    task_id,
                    segment_idx
                ],
            )
        })?;
        Ok(())
    }

    /// Delete all segment rows for a task (used when persisted offsets are
    /// stale — e.g. the remote file changed — and the download must restart
    /// from byte 0).
    pub fn delete_for_task(&self, task_id: &str) -> rusqlite::Result<()> {
        with_conn(self.db, |c| {
            c.execute("DELETE FROM segments WHERE task_id = ?1", params![task_id])
        })?;
        Ok(())
    }
}

/// Repository for the `settings` table.
pub struct SettingsRepo<'a> {
    db: &'a Arc<Mutex<Connection>>,
}

impl<'a> SettingsRepo<'a> {
    pub fn new(db: &'a Arc<Mutex<Connection>>) -> Self {
        Self { db }
    }

    /// Get a setting value. `Ok(None)` if missing.
    pub fn get(&self, key: &str) -> rusqlite::Result<Option<String>> {
        with_conn(self.db, |c| {
            let mut stmt = c.prepare("SELECT value FROM settings WHERE key = ?1")?;
            let mut rows = stmt.query(params![key])?;
            match rows.next()? {
                Some(r) => Ok(Some(r.get(0)?)),
                None => Ok(None),
            }
        })
    }

    /// Get all settings as a flat (key, value) list.
    pub fn all(&self) -> rusqlite::Result<Vec<(String, String)>> {
        with_conn(self.db, |c| {
            let mut stmt = c.prepare("SELECT key, value FROM settings ORDER BY key")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Upsert a setting.
    pub fn set(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        let now = now_ms();
        with_conn(self.db, |c| {
            c.execute(
                "INSERT INTO settings (key, value, updated_at) VALUES (?1,?2,?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                params![key, value, now],
            )
        })?;
        Ok(())
    }
}

/// Repository for the `events` table (append-only audit log).
pub struct EventsRepo<'a> {
    db: &'a Arc<Mutex<Connection>>,
}

impl<'a> EventsRepo<'a> {
    pub fn new(db: &'a Arc<Mutex<Connection>>) -> Self {
        Self { db }
    }

    pub fn append(&self, task_id: Option<&str>, kind: &str, payload: &str) -> rusqlite::Result<()> {
        with_conn(self.db, |c| {
            c.execute(
                "INSERT INTO events (task_id, kind, payload, ts) VALUES (?1,?2,?3,?4)",
                params![task_id, kind, payload, now_ms()],
            )
        })?;
        Ok(())
    }

    /// Tail events, newest first. Secondary sort by `id DESC` to give a
    /// deterministic order when multiple events share the same millisecond.
    pub fn tail(&self, limit: u32) -> rusqlite::Result<Vec<EventRow>> {
        with_conn(self.db, |c| {
            let mut stmt = c.prepare(
                "SELECT id, task_id, kind, payload, ts FROM events
                 ORDER BY ts DESC, id DESC LIMIT ?1",
            )?;
            let rows = stmt
                .query_map(params![limit], |r| {
                    Ok(EventRow {
                        id: r.get(0)?,
                        task_id: r.get(1)?,
                        kind: r.get(2)?,
                        payload: r.get(3)?,
                        ts: r.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Delete events older than `cutoff_ms`.
    pub fn prune_before(&self, cutoff_ms: Ts) -> rusqlite::Result<usize> {
        with_conn(self.db, |c| {
            let n = c.execute("DELETE FROM events WHERE ts < ?1", params![cutoff_ms])?;
            Ok(n)
        })
    }
}

// ---------------------------------------------------------------------------
// row → struct mappers
// ---------------------------------------------------------------------------

fn row_to_task(r: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRow> {
    let state_str: String = r.get("state")?;
    let state = TaskState::from_db_str(&state_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("unknown task state: {state_str}").into(),
        )
    })?;

    let qos_str: Option<String> = r.get("qos_override")?;
    let qos_override = qos_str.and_then(|s| QosOverride::from_db_str(&s));

    let accept_ranges: i64 = r.get("accept_ranges")?;
    Ok(TaskRow {
        id: r.get("id")?,
        url: r.get("url")?,
        filename: r.get("filename")?,
        save_path: r.get("save_path")?,
        total_bytes: r.get("total_bytes")?,
        downloaded_bytes: r.get("downloaded_bytes")?,
        state,
        etag: r.get("etag")?,
        last_modified: r.get("last_modified")?,
        accept_ranges: accept_ranges != 0,
        segments_requested: r.get("segments_requested")?,
        qos_override,
        extra_headers: r.get("extra_headers")?,
        error_message: r.get("error_message")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
        completed_at: r.get("completed_at")?,
    })
}

fn row_to_segment(r: &rusqlite::Row<'_>) -> rusqlite::Result<SegmentRow> {
    let state_str: String = r.get("state")?;
    let state = SegmentState::from_db_str(&state_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("unknown segment state: {state_str}").into(),
        )
    })?;
    Ok(SegmentRow {
        task_id: r.get("task_id")?,
        segment_idx: r.get("segment_idx")?,
        start_byte: r.get("start_byte")?,
        end_byte: r.get("end_byte")?,
        current_byte: r.get("current_byte")?,
        state,
        speed_bps: r.get("speed_bps")?,
        error_message: r.get("error_message")?,
        updated_at: r.get("updated_at")?,
    })
}

fn now_ms() -> Ts {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open;
    use tempfile::NamedTempFile;

    fn fresh_db() -> (NamedTempFile, Arc<Mutex<Connection>>) {
        let f = NamedTempFile::new().unwrap();
        let db = open(f.path()).unwrap();
        (f, db)
    }

    fn sample_task(id: &str) -> TaskRow {
        TaskRow {
            id: id.to_string(),
            url: "https://example.com/file.bin".into(),
            filename: "file.bin".into(),
            save_path: "/tmp/file.bin".into(),
            total_bytes: Some(1024 * 1024),
            downloaded_bytes: 0,
            state: TaskState::Queued,
            etag: None,
            last_modified: None,
            accept_ranges: false,
            segments_requested: 4,
            qos_override: None,
            extra_headers: None,
            error_message: None,
            created_at: 1_700_000_000_000,
            updated_at: 1_700_000_000_000,
            completed_at: None,
        }
    }

    #[test]
    fn task_insert_get_roundtrip() {
        let (_f, db) = fresh_db();
        let repo = TasksRepo::new(&db);
        let t = sample_task("abc");
        repo.insert(&t).unwrap();
        let loaded = repo.get("abc").unwrap().unwrap();
        assert_eq!(loaded, t);
    }

    #[test]
    fn get_missing_returns_none() {
        let (_f, db) = fresh_db();
        let repo = TasksRepo::new(&db);
        assert!(repo.get("nope").unwrap().is_none());
    }

    #[test]
    fn list_by_state_filters() {
        let (_f, db) = fresh_db();
        let repo = TasksRepo::new(&db);
        repo.insert(&sample_task("a")).unwrap();
        repo.insert(&sample_task("b")).unwrap();
        let queued = repo.list_by_state(Some(TaskState::Queued)).unwrap();
        assert_eq!(queued.len(), 2);
        let downloading = repo.list_by_state(Some(TaskState::Downloading)).unwrap();
        assert!(downloading.is_empty());
    }

    #[test]
    fn touch_updates_state_and_bytes() {
        let (_f, db) = fresh_db();
        let repo = TasksRepo::new(&db);
        repo.insert(&sample_task("a")).unwrap();
        repo.touch("a", TaskState::Downloading, 500, None).unwrap();
        let t = repo.get("a").unwrap().unwrap();
        assert_eq!(t.state, TaskState::Downloading);
        assert_eq!(t.downloaded_bytes, 500);
        assert!(t.error_message.is_none());
    }

    #[test]
    fn touch_to_complete_sets_completed_at() {
        let (_f, db) = fresh_db();
        let repo = TasksRepo::new(&db);
        repo.insert(&sample_task("a")).unwrap();
        repo.touch("a", TaskState::Complete, 1024 * 1024, None)
            .unwrap();
        let t = repo.get("a").unwrap().unwrap();
        assert_eq!(t.state, TaskState::Complete);
        assert!(t.completed_at.is_some());
    }

    #[test]
    fn delete_cascades_to_segments() {
        let (_f, db) = fresh_db();
        let tasks = TasksRepo::new(&db);
        let segs = SegmentsRepo::new(&db);
        tasks.insert(&sample_task("a")).unwrap();
        let s = SegmentRow {
            task_id: "a".into(),
            segment_idx: 0,
            start_byte: 0,
            end_byte: 100,
            current_byte: 0,
            state: SegmentState::Pending,
            speed_bps: 0,
            error_message: None,
            updated_at: 1,
        };
        segs.insert_batch(std::slice::from_ref(&s)).unwrap();
        assert_eq!(segs.list_for_task("a").unwrap().len(), 1);
        tasks.delete("a").unwrap();
        assert!(segs.list_for_task("a").unwrap().is_empty());
    }

    #[test]
    fn settings_get_set_upsert() {
        let (_f, db) = fresh_db();
        let repo = SettingsRepo::new(&db);
        // A seeded value should exist.
        assert_eq!(
            repo.get("bind").unwrap(),
            Some("127.0.0.1:7780".to_string())
        );
        repo.set("bind", "0.0.0.0:9000").unwrap();
        assert_eq!(repo.get("bind").unwrap(), Some("0.0.0.0:9000".to_string()));
        repo.set("custom_key", "hello").unwrap();
        assert_eq!(repo.get("custom_key").unwrap(), Some("hello".to_string()));
    }

    #[test]
    fn events_append_and_tail() {
        let (_f, db) = fresh_db();
        let repo = EventsRepo::new(&db);
        repo.append(Some("t1"), "task.created", "{\"v\":1}")
            .unwrap();
        repo.append(None, "qos.enabled", "{}").unwrap();
        let tail = repo.tail(10).unwrap();
        assert_eq!(tail.len(), 2);
        // newest first
        assert_eq!(tail[0].kind, "qos.enabled");
        assert_eq!(tail[1].kind, "task.created");
    }

    #[test]
    fn segments_insert_batch_and_list() {
        let (_f, db) = fresh_db();
        let tasks = TasksRepo::new(&db);
        let segs = SegmentsRepo::new(&db);
        tasks.insert(&sample_task("a")).unwrap();
        let rows: Vec<_> = (0..4)
            .map(|i| SegmentRow {
                task_id: "a".into(),
                segment_idx: i,
                start_byte: i * 256,
                end_byte: (i + 1) * 256 - 1,
                current_byte: 0,
                state: SegmentState::Pending,
                speed_bps: 0,
                error_message: None,
                updated_at: 1,
            })
            .collect();
        segs.insert_batch(&rows).unwrap();
        let loaded = segs.list_for_task("a").unwrap();
        assert_eq!(loaded.len(), 4);
        assert_eq!(loaded[0].segment_idx, 0);
        assert_eq!(loaded[3].segment_idx, 3);
    }
}
