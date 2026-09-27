-- 001_init.sql — initial schema for HyprFetch
--
-- Tables: settings, tasks, segments, events (audit log)
--
-- Conventions:
--   * All timestamps are stored as Unix milliseconds (i64) in UTC.
--   * IDs are stored as TEXT (UUID v7 strings, lexicographically sortable).
--   * State machines use TEXT CHECK constraints — keeps schema portable.

PRAGMA foreign_keys = ON;

-- ----------------------------------------------------------------------------
-- settings: key/value table for app-level config that the user can edit
--           from the UI. Re-read on PATCH.
-- ----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS settings (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL,
    updated_at  INTEGER NOT NULL
);

-- ----------------------------------------------------------------------------
-- tasks: one row per download.
-- ----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS tasks (
    id                  TEXT PRIMARY KEY,
    url                 TEXT NOT NULL,
    filename            TEXT NOT NULL,
    save_path           TEXT NOT NULL,
    total_bytes         INTEGER,        -- NULL if Content-Length unknown
    downloaded_bytes    INTEGER NOT NULL DEFAULT 0,
    state               TEXT NOT NULL DEFAULT 'queued'
                        CHECK (state IN ('queued','downloading','paused','complete','error','removed')),
    -- HTTP cache validators, used to detect if the remote resource changed
    -- between sessions. If either differs on a resume HEAD, we restart.
    etag                TEXT,
    last_modified       TEXT,
    accept_ranges       INTEGER NOT NULL DEFAULT 0,  -- 0/1 boolean
    -- Config the user picked at task creation; reused on resume.
    segments_requested  INTEGER NOT NULL DEFAULT 8,
    qos_override        TEXT,  -- NULL | 'auto' | 'force_on' | 'force_off'
    -- Optional request headers to send (User-Agent, Referer, Authorization, etc.)
    -- Stored as a JSON object.
    extra_headers       TEXT,
    error_message       TEXT,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    completed_at        INTEGER
);

CREATE INDEX IF NOT EXISTS idx_tasks_state       ON tasks(state);
CREATE INDEX IF NOT EXISTS idx_tasks_created_at  ON tasks(created_at DESC);

-- ----------------------------------------------------------------------------
-- segments: per-range state for multi-connection downloads.
--          (task_id, segment_idx) is the natural key.
-- ----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS segments (
    task_id         TEXT NOT NULL,
    segment_idx     INTEGER NOT NULL,
    start_byte      INTEGER NOT NULL,
    end_byte        INTEGER NOT NULL,
    current_byte    INTEGER NOT NULL DEFAULT 0,  -- bytes written so far
    state           TEXT NOT NULL DEFAULT 'pending'
                    CHECK (state IN ('pending','downloading','paused','complete','error')),
    speed_bps       INTEGER NOT NULL DEFAULT 0,
    error_message   TEXT,
    updated_at      INTEGER NOT NULL,
    PRIMARY KEY (task_id, segment_idx),
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_segments_state ON segments(state);

-- ----------------------------------------------------------------------------
-- events: append-only audit log for debugging. Old events pruned by
--         a periodic job (keep last 7 days).
-- ----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id      TEXT,    -- NULL for global events (qos change, settings change)
    kind        TEXT NOT NULL,    -- e.g. 'task.created', 'task.paused', 'qos.enabled'
    payload     TEXT NOT NULL,    -- JSON
    ts          INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_events_ts      ON events(ts DESC);
CREATE INDEX IF NOT EXISTS idx_events_task_id ON events(task_id);
