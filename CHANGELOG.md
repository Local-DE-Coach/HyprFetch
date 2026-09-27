# Changelog

All notable changes to HyprFetch are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
While pre-1.0, breaking API changes are allowed in MINOR bumps.

## [Unreleased]

### Added
- HTTP API (`hyprfetch-api` crate, axum 0.7):
  - `GET /healthz` — `{"status":"ok"}` for liveness probes
  - `GET /api/tasks?state=active|completed|all` — list with filter
  - `POST /api/tasks` — bulk create (up to 100 URLs per request), validates
    scheme is http/https, derives filename from URL, persists to SQLite,
    appends `task.created` audit event
  - `GET /api/tasks/:id` — full task detail with per-segment progress
  - `DELETE /api/tasks/:id` — removes task + cascades segments
  - `POST /api/tasks/:id/{pause,resume,cancel}` — state machine transitions
    with `409 Conflict` on invalid transitions
  - `GET /api/qos` / `PUT /api/qos` — read/write QoS state
  - `GET /api/settings` / `PATCH /api/settings` — flat key/value settings
  - Structured error responses: `{ "error": { "code": "...", "message": "..." } }`
    with stable codes (`invalid_url`, `ssrf_blocked`, `task_not_found`,
    `invalid_state_transition`, `invalid_request`, `internal_error`)
- `hyprfetch` binary:
  - `serve` command binds axum on `127.0.0.1:7780` (configurable via
    `--bind` or `HYPRFETCH_BIND` env var)
  - `doctor` command verifies db, settings, and pragma state
  - `--db-path`, `--download-dir`, `--segments` CLI flags + env var equivalents
  - Default db path: `${XDG_DATA_HOME:-~/.local/share}/hyprfetch/hyprfetch.db`
- `hyprfetch_db::open_in_memory()` — for tests; runs migrations + applies pragmas
- 9 new API integration tests (33 tests total across workspace)
