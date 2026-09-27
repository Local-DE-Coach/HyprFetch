# Changelog

All notable changes to HyprFetch are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
While pre-1.0, breaking API changes are allowed in MINOR bumps.

## [Unreleased]

### Dependencies (merged from 10 Dependabot branches, 2026-09-28)
- **cargo:** `thiserror` 1 → 2.0.21, `toml` 0.8 → 1.1.6+spec-1.1.0,
  `tokio-tungstenite` 0.21 → 0.24.0, `rusqlite` 0.32 → 0.40.2 (bundled SQLite
  refreshed), `governor` 0.6 → 0.10.4. No source changes required — the APIs
  HyprFetch uses are stable across each bump. Verified by the full suite
  (110/110) + live smoke (config parse, WAL open, download through the new
  token bucket, doctor).
- **CI actions:** `actions/checkout` 4 → 7, `actions/upload-artifact` 4 → 7,
  `actions/download-artifact` 4 → 8, `actions/stale` 9 → 11,
  `softprops/action-gh-release` 2 → 3. Workflows re-validated (YAML parse +
  `uses:` review).
- **Housekeeping:** the governor merge initially produced a duplicated
  `Cargo.lock` stanza (`hashbrown` twice); the lock was regenerated and
  verified. All 10 Dependabot branches deleted (remote now has only `main`),
  PRs #10–#19 closed.

### Fixed (found by the 2026-09-28 full audit: code review + live smoke test)
- **Downloads from servers without Range support failed entirely.** The
  segment worker required `206 Partial Content` even on the single-connection
  fallback path, so any server answering `200 OK` with the whole file (no
  `Accept-Ranges`) produced `only 0 of 1 segments completed`. The worker now
  accepts a 200 whole-file body when the segment starts at byte 0 and caps
  writes at the segment end. Regression test:
  `download_from_server_without_range_support`.
- **Finished tasks were never reaped from the engine's active-task map** —
  the map grew forever, `is_running()` stayed true after completion, and
  re-starting a finished task failed with "already running". Coordinator
  wrappers now remove their own entry; the map only holds running tasks
  (`finished_task_is_reaped_from_active_map`).
- **Queue double-spawn race could mark tasks `removed` spuriously** —
  spawning two coordinators for one task overwrote the map entry and dropped
  the live coordinator's only command sender (interpreted as cancel).
  `spawn_coordinator` is now check-and-insert atomic and each pump pass
  filters ids it already started.

### Added
- **`max_concurrent_tasks` is now enforced** (was a seeded-but-dead setting):
  a queue pump starts queued tasks oldest-first while running < limit and
  chains new starts when running tasks finish; 0/missing = unlimited;
  explicit user resumes bypass the cap.
  Tests: `queue_pump_respects_max_concurrent_tasks`,
  `queue_pump_unlimited_drains_queue`.
- **`POST /api/tasks/:id/retry`** — re-run an errored task: clears the
  error, re-queues, keeps persisted offsets (remote-change recheck on
  restart discards them if the file changed). Tests: 3 API cases + live
  smoke.
- **`DELETE /api/tasks/:id?delete_file=true`** now really deletes the
  (partial) file from disk (previously the param was silently ignored).
- **Bearer-token auth on non-loopback binds** — was documented in api.md but
  not implemented. `/api/*` + `/ws` now require `Authorization: Bearer
  <token>` (or `?access_token=` for browser WebSocket clients) when the
  daemon is bound off-loopback; `401` + `WWW-Authenticate` otherwise;
  `/healthz` + SPA stay open. Token auto-generates on first run to
  `~/.config/hyprfetch/token` (0600); resolution order: CLI `--api-token` >
  `HYPRFETCH_API_TOKEN` > config > settings > token file. Tests: 7 auth
  cases + live smoke (401/200/query-param).
- **Config file support** — `~/.config/hyprfetch/config.toml` (keys: `bind`,
  `db_path`, `download_dir`, `segments`, `allow_private`, `api_token`) with
  `--config` override; precedence CLI > env > file > default. Was documented
  in README/development.md but not implemented.
- **`user_agent` and `ssrf_block_private` settings are honored** at startup
  (were seeded-dead).
- **UI: Retry button** on errored tasks (Active + Finished lists) and the
  Finished ✕ now removes task + downloaded file (`?delete_file=true`);
  `ui/dist` rebuilt and committed.
- **In-session remote-change recheck** — on every (re)start of a task
  (pause → resume, retry), stored ETag/Last-Modified/size are compared
  against a fresh probe; stale offsets are discarded instead of corrupting
  the output (`resume_after_remote_change_mid_session_resets_offsets`).
- **`worktasks.md`** — project task board: per-feature-area status with test
  proof and verify commands, open bug tracker, backlog, and the task
  definition-of-done protocol for future sandboxes.
- docs/api.md: retry + delete_file documented; Auth section rewritten to
  match reality; settings enforcement table (which keys are live vs not yet).

## [0.2.0] — 2026-09-28

Packaging release: every release now ships ready-made Linux packages —
`.deb` for Ubuntu/Debian, `.rpm` for Fedora/RHEL, and a per-release
`PKGBUILD` for Arch — next to the binary tarballs. No engine changes; the
download code is identical to v0.1.1.

### Added
- **Linux packages in every release** (fast installation) — the release
  workflow now builds and attaches three package formats next to the binary
  tarballs:
  - `hyprfetch_<version>-1_amd64.deb` (Ubuntu/Debian) — built with
    `cargo-deb` from `[package.metadata.deb]` in `crates/hyprfetch/Cargo.toml`.
    Installs `/usr/bin/hyprfetch` + README/CHANGELOG/copyright under
    `/usr/share/doc/hyprfetch/`; the only runtime dependency is glibc
    (SQLite is bundled, TLS is rustls).
  - `hyprfetch-<version>-1.x86_64.rpm` (Fedora/RHEL) — built with
    `rpmbuild` from the new `packaging/rpm/hyprfetch.spec`, repackaging the
    x86_64 release tarball (binary + docs at the standard locations).
  - `PKGBUILD` (Arch Linux) — generated per release from the new
    `packaging/arch/PKGBUILD.bin.template` with the version and the sha256 of
    the x86_64 tarball substituted in, so Arch users can download one file
    from the release page and run `makepkg -si` for a fast, checksum-verified
    install (`hyprfetch-bin`).
- **docs/install.md** — full installation guide: .deb / .rpm / PKGBUILD /
  tarball / from-source paths, first-run flags, fd-limit note for heavy use.
- **docs/design.md** — design rationale ("why") for new contributors and
  sandboxes: language choice vs Go/Python, architecture, resume design,
  segmented downloads, QoS strategy, crate choices, prior art (aria2, gopeed),
  UI structure, WebSocket event contract, honest gotchas, and v1→v2 scope
  with per-item status markers.
- `.github/dependabot.yml` — weekly update PRs for Cargo dependencies and
  GitHub Actions.

### Changed
- Doc drift fixed: README + `docs/development.md` now point to the real UI
  directory (`crates/hyprfetch-api/ui/`, not the planned `web/`), README
  requires Rust 1.85+ (matching `rust-version`), and `docs/development.md`
  "Releases" lists the new deb/rpm/PKGBUILD artifacts and the
  tag-version-must-match-workspace-version rule.

## [0.1.1] — 2026-09-28

CI and engine reliability release. No new features — fixes the release
pipeline (aarch64 artifact was missing from v0.1.0) and two download
bugs found by sandbox testing of the v0.1.0 binary.

### Fixed
- **30-second total request timeout killed all long downloads** (sandbox test
  finding). `HttpClient` set reqwest's client-level `.timeout(30s)`, which covers
  the *entire* request including streaming the response body — so every segment
  worker aborted exactly 30 s in. Reproduced on thinkbroadband: 1 GB at ~16 MB/s
  died at ~455 MB (`only 0 of 8 segments completed`), the same task QoS-capped
  to 4 MiB/s died at ~130 MB — both at exactly ~32 s. The client now uses
  `connect_timeout(10s)` + `read_timeout(30s)` (idle-read gap) and no total
  timeout, so multi-minute transfers run to completion.
- **One dropped connection failed the whole task** — segment workers now retry
  transient failures (up to `MAX_SEGMENT_ATTEMPTS = 6`, exponential backoff
  1 s → 15 s) and resume from the last written byte offset. Retryable:
  transport errors, mid-body EOF, HTTP 5xx / 429. Non-retryable: disk I/O
  errors, SSRF violations, ignored Range headers, 4xx. Observed live: 4 of 8
  segments were dropped ~150 s into a download; with retry the task completed
  without user intervention.

### Changed
- `docs/api.md`: list-tasks example now matches the real `TaskDto`
  (`segments_requested`, epoch timestamps; no `speed_bps`/`eta_sec`/`segments_active`),
  and the not-implemented `POST /api/tasks/:id/retry` endpoint is marked as planned.

### Added
- **README: "Sandbox test results (2026-09-28)"** — full test data of the
  1 GB / 5 GB thinkbroadband runs (throughput, RSS, integrity sha256
  verification, resume-across-restart proof) and the analysis that led to the
  two engine fixes above.
- **Multi-sandbox coordination** — repo-root `worklog.md` records what each
  sandbox/agent did per commit; every commit is expected to update both
  `CHANGELOG.md` and `worklog.md` (protocol documented in the file and in
  `CONTRIBUTING.md`).

### Tests
- 91 → 93: retryability classification (`is_retryable`) and backoff sequencing
  (`backoff_delay`), both pure unit tests.

### Fixed (CI)
- **aarch64 release build** — the release workflow now sets
  `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc` (plus
  `CC_`/`AR_` for the C dependencies `ring` and `libsqlite3-sys`). Previously the
  aarch64 job linked with the host `cc` and rustc's self-contained lld, which failed
  with `rust-lld: error: --fix-cortex-a53-843419 is only supported on AArch64`.
- **release packaging on `workflow_dispatch`** — archive names and the published
  release now derive from the `tag` input (`inputs.tag || github.ref_name`) instead of
  `GITHUB_REF_NAME`, so dispatching from a branch no longer produces
  `hyprfetch-main-*` archives / a `main` release.

## [0.1.0] — 2026-09-27

First user-facing release: segmented downloads, one shared QoS bucket,
live WebSocket events, embedded browser UI, and crash-safe resume.

### Added
- **QoS rate limiter** (PR #5) — the differentiator:
  - `QosLimiter::acquire()` is now a real governor-backed token bucket —
    `acquire(bytes)` awaits actual download budget; oversized chunks are
    split into capacity-sized acquisitions automatically
  - **One shared limiter across all active tasks**: the engine owns a
    single engine-wide bucket restored from the `qos_enabled` /
    `qos_target_bps` settings, so the *aggregate* bandwidth of the whole
    daemon is capped, not each task separately
  - `PUT /api/qos` applies the new rate live to the running engine
  - Per-task `qos_override: force_off` bypasses the shared bucket
  - Runtime retune atomically swaps in a fresh full bucket; bucket
    capacity equals one second of the target rate
- **WebSocket event fan-out** (PR #6):
  - `GET /ws` upgrades and streams JSON events: `task:progress`
    (downloaded/total/speed_bps), `task:state` (state/error),
    `global:speed` (aggregate B/s + active task count)
  - Core `EventBus` (tokio broadcast, 1024-deep per subscriber); slow
    clients skip missed events — progress is self-correcting since every
    event carries absolute totals
  - Background speed aggregator emits `global:speed` every 1s while tasks
    are active, with one final zero event on the active→idle transition
  - `task:progress` is throttled to one event per 500ms per task
- **Embedded web UI** (PR #7):
  - Minimal Svelte 4 SPA compiled into the binary via rust-embed:
    active list with live progress bars, add-task modal (multi-URL,
    save dir, segment count), QoS bandwidth toggle
  - Served at `/`: hashed assets cached `immutable`, extension-less
    paths fall back to the SPA, `/api/*` keeps proper JSON 404s
  - `ui/dist` is committed — `cargo build` needs no Node toolchain;
    rebuild with `npm run build` (dev proxy documented in
    `crates/hyprfetch-api/ui/README.md`)
- **Resume persistence** (PR #8):
  - `hyprfetch serve` runs a startup resume pass: every incomplete task
    (queued/downloading/paused) is reloaded, HEAD-probed, and its stored
    ETag / Last-Modified / Content-Length compared with the remote
  - Remote unchanged → workers restart **from persisted segment offsets**
  - Remote changed → offsets discarded, download restarts from byte 0
  - `downloading` rows (crash leftovers) are normalized to paused first
  - Probe failure marks the task `error` with a reason (retryable);
    the pass never aborts on individual task failures
  - Offsets also reset when the local file is missing/truncated vs the
    stored total
  - `SegmentsRepo::delete_for_task()` for stale offset cleanup

### Tests
- Total: 91 tests across the workspace, all passing (64 → 91)
  - QoS: 12 (throttle rate, oversized-chunk splitting, shared budget
    across concurrent workers, retune swap, force_off bypass, API
    live-apply, settings restore)
  - Events: 5 unit tests (bus fan-out, idle transition, serialization)
    + 1 E2E over a real TCP WebSocket (upgrade, live 2 MiB 4-segment
    download under QoS, all three event kinds observed)
  - UI serving: 3 (index, hashed asset + API 404, deep-link fallback)
  - Resume: 6 (byte-exact offset restart, stale-ETag reset with corrupt
    local file, crash-leftover normalization, probe-failure marking,
    empty pass, DB cascade helper)

### Previously added (PR #4 — download engine)
- **Segmented download engine** (`hyprfetch-core` crate):
  - `Engine` — orchestrator that owns the task table, spawns segment workers
    per task, aggregates progress via mpsc channels, debounces SQLite
    persistence (500ms), and handles pause/cancel via a command channel
  - `HttpClient` — reqwest wrapper with HEAD probe, Range fetch, ETag /
    Last-Modified / Accept-Ranges / Content-Length parsing, redirect
    following (max 5 hops), SSRF re-check on final URL
  - `SegmentWorker` — fetches one byte range, writes via `pwrite` (positional
    write) to a shared file `Arc<File>` so segments don't need locking
  - `planner::split()` — splits total bytes into N non-overlapping segments
    (last absorbs remainder)
  - `SsrfPolicy` + `check_url()` — blocks private/loopback/link-local/
    CGNAT/unique-local IPv4+IPv6 ranges; resolves DNS for domain hosts
  - `open_target_file()` — opens file with `ftruncate` pre-allocation
- `Engine::with_ssrf_policy()` — for tests and the `--allow-private` CLI flag
- `hyprfetch-api::make_state()` helper — constructs `AppState` with engine
- `hyprfetch serve` now accepts `--allow-private` flag (default: false).
  When false (production default), SSRF protection blocks loopback and
  private IPs. Set `HYPRFETCH_ALLOW_PRIVATE=true` to disable for testing.
- API integration: `POST /api/tasks` auto-starts the engine;
  `POST /api/tasks/:id/{pause,resume,cancel}` propagate to the engine

### Previously added (PR #3 — HTTP API)
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
