//! SQLite persistence layer: connection pool, migrations, repositories.
//!
//! Stub. Real schema lands in `feature/sqlite-schema`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::path::Path;

use rusqlite::Connection;

/// Open (or create) the database file, run migrations, configure WAL mode.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    // Keep page cache small — we target low RAM.
    conn.pragma_update(None, "cache_size", "-256")?; // 256 KB
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn open_inits_wal_mode() {
        let f = NamedTempFile::new().unwrap();
        let conn = open(f.path()).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[test]
    fn foreign_keys_are_on() {
        let f = NamedTempFile::new().unwrap();
        let conn = open(f.path()).unwrap();
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
    }

    #[test]
    fn open_creates_file_if_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.db");
        assert!(!path.exists());
        let _conn = open(&path).unwrap();
        assert!(path.exists());
    }
}
