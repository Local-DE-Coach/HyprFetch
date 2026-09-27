# Changelog

All notable changes to HyprFetch are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
While pre-1.0, breaking API changes are allowed in MINOR bumps.

## [Unreleased]

### Added
- SQLite persistence layer (`hyprfetch-db` crate):
  - `migrations/001_init.sql` — schema for `tasks`, `segments`, `settings`, `events` tables
  - `migrations/002_seed_settings.sql` — default app settings (bind, download_dir, segments_default, qos_*, etc.)
  - Migration runner with `schema_migrations` tracking table; idempotent
  - `TaskState`, `SegmentState`, `QosOverride` enums with serde + DB-string round-trip
  - `TasksRepo` (insert, get, list_by_state, touch, update_cache_validators, delete)
  - `SegmentsRepo` (insert_batch, list_for_task, touch)
  - `SettingsRepo` (get, set, all)
  - `EventsRepo` (append, tail, prune_before)
  - 16 new unit tests covering migrations, repos, and state machine round-trips
- `open()` configures WAL mode, foreign_keys=ON, 256 KB page cache (RAM target preserved)
