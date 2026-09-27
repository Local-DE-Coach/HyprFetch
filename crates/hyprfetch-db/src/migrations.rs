//! Embedded migrations.
//!
//! Migrations are forward-only, numbered `NNN_description.sql`, and applied
//! in order. A `schema_migrations` table tracks which ones have been applied.

use include_dir::Dir;
use rusqlite::Connection;

/// Embedded migration SQL files at compile time.
static MIGRATIONS: Dir<'_> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/migrations");

/// Run all pending migrations.
pub fn run(conn: &Connection) -> rusqlite::Result<()> {
    // Ensure the migrations tracking table exists.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL
        );",
    )?;

    let mut entries: Vec<_> = MIGRATIONS
        .entries()
        .iter()
        .filter_map(|e| e.as_file())
        .filter_map(|f| {
            let name = f.path().file_name()?.to_string_lossy().to_string();
            let version: i64 = name.split('_').next()?.parse().ok()?;
            Some((version, name, f.clone()))
        })
        .collect();
    entries.sort_by_key(|(v, _, _)| *v);

    let latest_applied: Option<i64> = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
            r.get(0)
        })
        .ok()
        .flatten();

    for (version, name, file) in entries {
        if Some(version) <= latest_applied {
            continue;
        }
        let sql = file.contents_utf8().ok_or_else(|| {
            rusqlite::Error::ToSqlConversionFailure(
                format!("migration {name} is not valid UTF-8").into(),
            )
        })?;
        tracing::info!(version, name, "applying migration");
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            rusqlite::params![version, now_ms()],
        )?;
        tx.commit()?;
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

// Compile-time check that include_dir is in the deps tree.
#[allow(dead_code)]
const _ASSERT_INCLUDE_DIR: Dir<'static> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/migrations");

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn fresh() -> (NamedTempFile, Connection) {
        let f = NamedTempFile::new().unwrap();
        let conn = Connection::open(f.path()).unwrap();
        (f, conn)
    }

    #[test]
    fn runs_migrations_on_fresh_db() {
        let (_f, conn) = fresh();
        run(&conn).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert!(
            count >= 2,
            "expected at least 2 migrations applied, got {count}"
        );

        // Settings table should be seeded.
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM settings", [], |r| r.get(0))
            .unwrap();
        assert!(n >= 10, "expected seeded settings, got {n}");
    }

    #[test]
    fn migrations_are_idempotent() {
        let (_f, conn) = fresh();
        run(&conn).unwrap();
        // Running again should be a no-op.
        run(&conn).unwrap();
    }

    #[test]
    fn tables_exist_after_migration() {
        let (_f, conn) = fresh();
        run(&conn).unwrap();
        for table in ["tasks", "segments", "settings", "events"] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    rusqlite::params![table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "table {table} missing");
        }
    }
}
