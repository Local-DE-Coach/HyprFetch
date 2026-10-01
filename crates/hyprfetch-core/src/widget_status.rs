//! Desktop-widget status file (v0.4.8; `path` added in v0.5.1).
//!
//! Quickshell widgets (illogical-impulse and friends) cannot poll the
//! HTTP API without burning RAM, but they CAN watch a file. So the daemon
//! mirrors a tiny snapshot of the download state into
//!
//! ```text
//! $XDG_DATA_HOME/download-manager/status.json   (default ~/.local/share/…)
//! ```
//!
//! with this exact schema (fields the widget renders; extra keys may be
//! added later — widgets must ignore unknown keys):
//!
//! ```json
//! {
//!   "active_downloads": [
//!     { "id": "t_ab12", "filename": "arch.iso", "progress": 45.5,
//!       "speed": "2.5 MB/s", "eta": "00:02:15", "state": "downloading",
//!       "path": "/home/u/Downloads/arch.iso" }
//!   ],
//!   "recent_downloads": [
//!     { "id": "t_cd34", "filename": "video.mp4", "status": "completed",
//!       "timestamp": 1696000000, "path": "/home/u/Downloads/video.mp4" }
//!   ],
//!   "last_completed": { "filename": "video.mp4", "timestamp": 1696000000 }
//! }
//! ```
//!
//! Design constraints (the whole point of this module):
//! - **Event-driven, zero polling.** The writer wakes on engine events
//!   (`task:progress`, `task:state`) — never on a timer.
//! - **Throttled writes.** Progress storms coalesce: at most one write per
//!   [`THROTTLE`] window (500 ms), and only when content actually changed.
//! - **Watcher-friendly writes.** The file is written in place
//!   (truncate + single `write(2)`). Atomic tmp+rename replaces the inode
//!   and silently breaks `QFileSystemWatcher`-based watches on some
//!   kernels — the one failure mode a watching widget must never hit.
//! - **Trivial RAM.** One small map + one small vec; a few KiB total.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;

use hyprfetch_db::{TaskState, TasksRepo};

use crate::events::EngineEvent;

/// Minimum interval between two writes of the status file.
pub const THROTTLE: Duration = Duration::from_millis(500);

/// How many entries `recent_downloads` keeps (the popup shows 3–5).
pub const RECENT_LIMIT: usize = 5;

/// The widget's status file directory (`$XDG_DATA_HOME/download-manager`).
pub fn status_dir() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home).join(".local").join("share")
        });
    base.join("download-manager")
}

/// Full path of the status file.
pub fn status_file_path() -> PathBuf {
    status_dir().join("status.json")
}

/// One row of `active_downloads`.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveEntry {
    pub filename: String,
    /// 0.0 – 100.0 (unknown total ⇒ 0.0; the widget still shows speed).
    pub progress: f64,
    pub speed: String,
    pub eta: String,
    /// `downloading` | `queued` | `paused` (extra info for the widget).
    pub state: &'static str,
    /// Absolute save path (v0.5.1) — the widget's Open / Open Location
    /// buttons use it; empty means "let the widget guess".
    pub path: String,
}

/// One row of `recent_downloads`.
#[derive(Debug, Clone, PartialEq)]
pub struct RecentEntry {
    pub id: String,
    pub filename: String,
    /// `completed` | `error`.
    pub status: &'static str,
    /// Unix SECONDS (the widget schema uses seconds, not millis).
    pub timestamp: i64,
    /// Absolute save path (v0.5.1) — see [`ActiveEntry::path`].
    pub path: String,
}

/// The full in-memory snapshot mirrored to the status file.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct WidgetStatus {
    /// Keyed by task id (BTreeMap ⇒ deterministic, testable ordering).
    pub active: BTreeMap<String, ActiveEntry>,
    /// Newest first, capped at [`RECENT_LIMIT`].
    pub recent: Vec<RecentEntry>,
    /// `(filename, unix-seconds)` set whenever a download finishes OK.
    pub last_completed: Option<(String, i64)>,
}

/// `2.5 MB/s` style — matches the widget schema example.
pub fn format_speed(bps: u64) -> String {
    let v = bps as f64;
    if v >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GB/s", v / (1024.0 * 1024.0 * 1024.0))
    } else if v >= 1024.0 * 1024.0 {
        format!("{:.1} MB/s", v / (1024.0 * 1024.0))
    } else if v >= 1024.0 {
        format!("{:.1} KB/s", v / 1024.0)
    } else {
        format!("{bps} B/s")
    }
}

/// `00:02:15` style — `--:--:--` while size or speed is unknown.
pub fn format_eta(total: Option<i64>, downloaded: i64, bps: u64) -> String {
    let Some(total) = total.filter(|t| *t > 0) else {
        return "--:--:--".into();
    };
    let remaining = (total - downloaded).max(0);
    if bps == 0 || remaining == 0 {
        return "--:--:--".into();
    }
    let secs = (remaining as u64).div_ceil(bps);
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn progress_frac(downloaded: i64, total: Option<i64>) -> f64 {
    match total {
        Some(t) if t > 0 => ((downloaded as f64) / (t as f64) * 1000.0).floor() / 10.0,
        _ => 0.0,
    }
}

impl WidgetStatus {
    /// Serialize to the exact widget schema (compact JSON — file stays tiny).
    pub fn to_json(&self) -> serde_json::Value {
        let active: Vec<serde_json::Value> = self
            .active
            .iter()
            .map(|(id, e)| {
                json!({
                    "id": id,
                    "filename": e.filename,
                    "progress": e.progress,
                    "speed": e.speed,
                    "eta": e.eta,
                    "state": e.state,
                    "path": e.path,
                })
            })
            .collect();
        let recent: Vec<serde_json::Value> = self
            .recent
            .iter()
            .map(|r| {
                json!({
                    "id": r.id,
                    "filename": r.filename,
                    "status": r.status,
                    "timestamp": r.timestamp,
                    "path": r.path,
                })
            })
            .collect();
        let last = self
            .last_completed
            .as_ref()
            .map(|(f, ts)| json!({ "filename": f, "timestamp": ts }));
        json!({
            "active_downloads": active,
            "recent_downloads": recent,
            "last_completed": last,
        })
    }

    /// Insert at the front of `recent_downloads`, dedup by id, cap length.
    pub fn push_recent(&mut self, entry: RecentEntry) {
        self.recent.retain(|r| r.id != entry.id);
        self.recent.insert(0, entry);
        self.recent.truncate(RECENT_LIMIT);
    }

    /// Drop a task from `active_downloads`.
    pub fn remove_active(&mut self, id: &str) -> bool {
        self.active.remove(id).is_some()
    }
}

/// Rebuild the whole snapshot from the DB (startup + event-lag recovery).
pub fn resync_from_db(db: &Arc<std::sync::Mutex<rusqlite::Connection>>) -> WidgetStatus {
    let mut st = WidgetStatus::default();
    let repo = TasksRepo::new(db);
    let Ok(rows) = repo.list_by_state(None) else {
        return st;
    };
    for row in &rows {
        match row.state {
            TaskState::Queued | TaskState::Downloading | TaskState::Paused => {
                st.active.insert(
                    row.id.clone(),
                    ActiveEntry {
                        filename: row.filename.clone(),
                        progress: progress_frac(row.downloaded_bytes, row.total_bytes),
                        speed: format_speed(0),
                        eta: "--:--:--".into(),
                        state: match row.state {
                            TaskState::Queued => "queued",
                            TaskState::Paused => "paused",
                            _ => "downloading",
                        },
                        path: row.save_path.clone(),
                    },
                );
            }
            TaskState::Complete | TaskState::Error => {
                if let Some(ts) = row.completed_at {
                    st.push_recent(RecentEntry {
                        id: row.id.clone(),
                        filename: row.filename.clone(),
                        status: if row.state == TaskState::Complete {
                            "completed"
                        } else {
                            "error"
                        },
                        timestamp: ts.div_euclid(1000),
                        path: row.save_path.clone(),
                    });
                }
            }
            TaskState::Removed => {}
        }
    }
    // `push_recent` keeps newest-first by insertion order; a DB resync walks
    // rows in arbitrary order, so re-sort by timestamp and re-cap.
    st.recent.sort_by_key(|a| std::cmp::Reverse(a.timestamp));
    st.recent.truncate(RECENT_LIMIT);
    if let Some(newest) = st.recent.iter().find(|r| r.status == "completed") {
        st.last_completed = Some((newest.filename.clone(), newest.timestamp));
    }
    st
}

/// Apply one engine event; returns true when the snapshot changed.
pub fn apply_event(
    st: &mut WidgetStatus,
    ev: &EngineEvent,
    db: &Arc<std::sync::Mutex<rusqlite::Connection>>,
) -> bool {
    match ev.event {
        "task:progress" => {
            let Some(id) = ev.task_id.as_deref() else {
                return false;
            };
            let downloaded = ev
                .payload
                .get("downloaded_bytes")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let total = ev.payload.get("total_bytes").and_then(|v| v.as_i64());
            let bps = ev
                .payload
                .get("speed_bps")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let Some(entry) = st.active.get_mut(id) else {
                return false; // not tracked (yet) — a state event will add it
            };
            entry.progress = progress_frac(downloaded, total);
            entry.speed = format_speed(bps);
            entry.eta = format_eta(total, downloaded, bps);
            entry.state = "downloading";
            true
        }
        "task:state" => {
            let Some(id) = ev.task_id.as_deref() else {
                return false;
            };
            let state = ev
                .payload
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let row = TasksRepo::new(db).get(id).ok().flatten();
            match state.as_str() {
                "queued" | "downloading" | "paused" => {
                    let filename = row
                        .as_ref()
                        .map(|r| r.filename.clone())
                        .unwrap_or_else(|| id.to_string());
                    let (progress, paused) = match &row {
                        Some(r) => (
                            progress_frac(r.downloaded_bytes, r.total_bytes),
                            r.state == TaskState::Paused,
                        ),
                        None => (0.0, state == "paused"),
                    };
                    let entry = ActiveEntry {
                        filename,
                        progress,
                        speed: format_speed(0),
                        eta: "--:--:--".into(),
                        state: if state == "queued" {
                            "queued"
                        } else if paused {
                            "paused"
                        } else {
                            "downloading"
                        },
                        path: row
                            .as_ref()
                            .map(|r| r.save_path.clone())
                            .unwrap_or_default(),
                    };
                    if st.active.get(id) != Some(&entry) {
                        st.active.insert(id.to_string(), entry);
                        true
                    } else {
                        false
                    }
                }
                "complete" => {
                    st.remove_active(id);
                    let (filename, ts, path) = row
                        .map(|r| {
                            (
                                r.filename,
                                r.completed_at.unwrap_or_else(|| now_secs() * 1000),
                                r.save_path,
                            )
                        })
                        .unwrap_or_else(|| (id.to_string(), now_secs() * 1000, String::new()));
                    st.push_recent(RecentEntry {
                        id: id.to_string(),
                        filename: filename.clone(),
                        status: "completed",
                        timestamp: ts.div_euclid(1000),
                        path,
                    });
                    st.last_completed = Some((filename, ts.div_euclid(1000)));
                    true
                }
                "error" => {
                    st.remove_active(id);
                    let (filename, ts, path) = row
                        .map(|r| {
                            (
                                r.filename,
                                r.completed_at.unwrap_or_else(|| now_secs() * 1000),
                                r.save_path,
                            )
                        })
                        .unwrap_or_else(|| (id.to_string(), now_secs() * 1000, String::new()));
                    st.push_recent(RecentEntry {
                        id: id.to_string(),
                        filename,
                        status: "error",
                        timestamp: ts.div_euclid(1000),
                        path,
                    });
                    true
                }
                _ => st.remove_active(id),
            }
        }
        _ => false,
    }
}

/// Write the snapshot to `path` (in place — see the module docs).
pub fn write_status(path: &std::path::Path, st: &WidgetStatus) -> std::io::Result<()> {
    let body = st.to_json().to_string();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    f.write_all(body.as_bytes())?;
    f.flush()
}

/// Spawn the status-file writer. Must be called from a tokio context
/// (the serve path). Never fails the caller: file-writing problems are
/// logged and retried on the next change — the daemon must not care.
pub fn spawn(db: Arc<std::sync::Mutex<rusqlite::Connection>>, engine: Arc<crate::Engine>) {
    let mut rx = engine.subscribe();
    tokio::spawn(async move {
        let dir = status_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(dir = %dir.display(), error = %e, "widget status dir");
        }
        let path = status_file_path();
        tracing::info!(path = %path.display(), "widget status file active");

        // Initial snapshot so a freshly started widget always reads valid JSON.
        let mut st = resync_from_db(&db);
        if let Err(e) = write_status(&path, &st) {
            tracing::warn!(error = %e, "initial widget status write failed");
        }
        let mut last_write = Instant::now();
        let mut dirty = false;

        loop {
            // Wait for the next event OR the pending throttled-write deadline —
            // no polling: when nothing is dirty and no events arrive, this
            // parks forever.
            let deadline = dirty.then(|| last_write + THROTTLE);
            tokio::select! {
                biased;
                _ = async {
                    match deadline {
                        Some(d) => tokio::time::sleep_until(d.into()).await,
                        None => std::future::pending::<()>().await,
                    }
                } => {}
                ev = rx.recv() => {
                    match ev {
                        Ok(event) => dirty |= apply_event(&mut st, &event, &db),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(missed = n, "widget status lagged — resyncing");
                            st = resync_from_db(&db);
                            dirty = true;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
            if dirty && last_write.elapsed() >= THROTTLE {
                if let Err(e) = write_status(&path, &st) {
                    tracing::warn!(error = %e, "widget status write failed");
                }
                dirty = false;
                last_write = Instant::now();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active(progress: f64, bps: u64) -> ActiveEntry {
        ActiveEntry {
            filename: "arch.iso".into(),
            progress,
            speed: format_speed(bps),
            eta: format_eta(Some(1_000_000), 455_000, bps),
            state: "downloading",
            path: "/home/u/Downloads/arch.iso".into(),
        }
    }

    #[test]
    fn speed_strings_match_widget_schema_style() {
        assert_eq!(format_speed(0), "0 B/s");
        assert_eq!(format_speed(512), "512 B/s");
        assert_eq!(format_speed(2_500_000), "2.4 MB/s");
        assert_eq!(format_speed(2_621_440), "2.5 MB/s"); // 2.5 * 1024 * 1024
        assert_eq!(format_speed(1_073_741_824), "1.0 GB/s");
    }

    #[test]
    fn eta_strings_are_hhmmss_or_placeholder() {
        // 90_000 bytes left at 1_000 B/s = 90s.
        assert_eq!(format_eta(Some(100_000), 10_000, 1_000), "00:01:30");
        // 2h.
        assert_eq!(format_eta(Some(7_200_000), 0, 1_000), "02:00:00");
        assert_eq!(format_eta(None, 10, 1_000), "--:--:--");
        assert_eq!(format_eta(Some(1_000), 1_000, 1_000), "--:--:--");
        assert_eq!(format_eta(Some(1_000), 0, 0), "--:--:--");
    }

    #[test]
    fn json_has_exact_widget_schema_keys() {
        let mut st = WidgetStatus::default();
        st.active.insert("t1".into(), active(45.5, 2_621_440));
        st.recent.insert(
            0,
            RecentEntry {
                id: "t2".into(),
                filename: "video.mp4".into(),
                status: "completed",
                timestamp: 1_696_000_000,
                path: "/home/u/Downloads/video.mp4".into(),
            },
        );
        st.last_completed = Some(("video.mp4".into(), 1_696_000_000));

        let v = st.to_json();
        let obj = v.as_object().unwrap();
        // serde_json sorts map keys alphabetically — the widget only cares
        // that all three schema members exist.
        assert_eq!(
            obj.keys().collect::<Vec<_>>(),
            vec!["active_downloads", "last_completed", "recent_downloads"]
        );
        let a = &obj["active_downloads"].as_array().unwrap()[0];
        for k in [
            "id", "filename", "progress", "speed", "eta", "state", "path",
        ] {
            assert!(a.get(k).is_some(), "active entry missing {k}");
        }
        assert_eq!(a["id"], "t1");
        assert_eq!(a["filename"], "arch.iso");
        assert_eq!(a["speed"], "2.5 MB/s");
        assert_eq!(a["path"], "/home/u/Downloads/arch.iso");
        let r = &obj["recent_downloads"].as_array().unwrap()[0];
        for k in ["id", "filename", "status", "timestamp", "path"] {
            assert!(r.get(k).is_some(), "recent entry missing {k}");
        }
        assert_eq!(r["id"], "t2");
        assert_eq!(r["status"], "completed");
        assert_eq!(r["timestamp"], 1_696_000_000);
        let l = obj["last_completed"].as_object().unwrap();
        assert_eq!(l["filename"], "video.mp4");
        assert_eq!(l["timestamp"], 1_696_000_000);
    }

    #[test]
    fn json_serializes_empty_state_with_last_completed_null() {
        let v = WidgetStatus::default().to_json();
        assert_eq!(v["active_downloads"].as_array().unwrap().len(), 0);
        assert_eq!(v["recent_downloads"].as_array().unwrap().len(), 0);
        assert!(v["last_completed"].is_null());
    }

    #[test]
    fn recent_is_capped_deduped_and_newest_first() {
        let mut st = WidgetStatus::default();
        for i in 0..8 {
            st.push_recent(RecentEntry {
                id: format!("t{i}"),
                filename: format!("f{i}"),
                status: "completed",
                timestamp: i,
                path: format!("/tmp/f{i}"),
            });
        }
        assert_eq!(st.recent.len(), RECENT_LIMIT);
        assert_eq!(st.recent[0].filename, "f7");
        // Re-adding an existing id moves it to the front instead of duping.
        st.push_recent(RecentEntry {
            id: "t3".into(),
            filename: "f3".into(),
            status: "completed",
            timestamp: 99,
            path: "/tmp/f3".into(),
        });
        assert_eq!(st.recent.len(), RECENT_LIMIT);
        assert_eq!(st.recent[0].id, "t3");
        assert!(st.recent.iter().filter(|r| r.id == "t3").count() == 1);
    }

    #[test]
    fn write_status_produces_parseable_json_in_place() {
        let tmp = std::env::temp_dir().join(format!("hyprfetch-wtest-{}.json", std::process::id()));
        let mut st = WidgetStatus::default();
        st.active.insert("t1".into(), active(10.0, 100));
        write_status(&tmp, &st).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&tmp).unwrap()).unwrap();
        assert_eq!(v["active_downloads"][0]["filename"], "arch.iso");
        // Overwrite (truncate path) — second write must also parse.
        st.active.clear();
        write_status(&tmp, &st).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&tmp).unwrap()).unwrap();
        assert_eq!(v["active_downloads"].as_array().unwrap().len(), 0);
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn status_path_uses_xdg_data_home() {
        std::env::set_var("XDG_DATA_HOME", "/tmp/hf-xdg-test");
        assert_eq!(
            status_file_path(),
            PathBuf::from("/tmp/hf-xdg-test/download-manager/status.json")
        );
        std::env::remove_var("XDG_DATA_HOME");
    }
}
