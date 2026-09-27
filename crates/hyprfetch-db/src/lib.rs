//! SQLite persistence layer: connection, migrations, repositories.
//!
//! ## Schema layout
//!
//! - `tasks` — one row per download
//! - `segments` — per-byte-range state for multi-connection downloads
//! - `settings` — key/value app config (editable from UI)
//! - `events` — append-only audit log
//!
//! See `migrations/` for the SQL.

#![forbid(unsafe_code)]

mod migrations;
mod repo;
pub mod schema;

pub use repo::{EventRow, EventsRepo, SegmentRow, SegmentsRepo, SettingsRepo, TaskRow, TasksRepo};
pub use schema::TaskState;

use std::path::Path;
use std::sync::Arc;

use rusqlite::Connection;

/// Open (or create) the database file, run migrations, configure pragmas.
///
/// Returns a single connection. For higher concurrency wrap in a pool
/// (e.g. `r2d2` or `deadpool-sqlite`); for now we use a `Mutex<Connection>`
/// since our access pattern is mostly sequential writes from the engine.
pub fn open(path: &Path) -> rusqlite::Result<Arc<Mutex<Connection>>> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    // Keep page cache small — we target low RAM.
    conn.pragma_update(None, "cache_size", "-256")?; // 256 KB
    migrations::run(&conn)?;
    Ok(Arc::new(Mutex::new(conn)))
}

use std::sync::Mutex;
