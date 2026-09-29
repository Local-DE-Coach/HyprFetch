# Work Tasks — HyprFetch project task board

This file is the **single task-management board** for HyprFetch. It lists
every feature area, what is done, what is missing, and what tests guard it —
so any developer, agent, or sandbox can pick up work, check when a task is
really finished, and know what is still broken or not built yet.

It works together with [`CHANGELOG.md`](CHANGELOG.md) (what changed, per
release) and [`worklog.md`](worklog.md) (who did what, per session). When in
doubt about the *current truth* of a feature, trust the code + the test
suite, then this file.

**Last full audit:** 2026-09-28 (post-v0.2.0) — full code review + live
server smoke test + unit/integration suite. Test count after fixes: **110
passing** (was 93). Bugs found & fixed in that audit are marked ✅ below;
everything found still missing is marked ⬜ or 🐛 and listed in the Bug
tracker / Backlog.

---

## How to use this file (protocol)

1. **Before starting a task** — read its row, the linked docs, and the latest
   `worklog.md` entries. Check the box is actually unchecked in `main`.
2. **Definition of done for ANY task** — all of these, or the task is not
   done:
   - [ ] Code implemented (no stubs, no dead settings left behind)
   - [ ] Tests added/updated (`cargo test` green; UI changes rebuilt into
     `crates/hyprfetch-api/ui/dist/` and committed)
   - [ ] `cargo fmt --all` + `cargo clippy --all-targets -- -D warnings` clean
   - [ ] Docs touched where behavior changed (`docs/api.md`, `README.md`,
     `docs/*.md`)
   - [ ] `CHANGELOG.md` entry under `[Unreleased]`
   - [ ] `worklog.md` session entry appended (top of Sessions)
   - [ ] Checkbox flipped here **with a one-line proof** (test name, command,
     or live check) in the row's *Proof* slot
3. **Do not delete rows.** Strike through abandoned ideas with ~~strikethrough~~
   and note why.
4. Every row has a **Verify** command — the fastest way to confirm the task
   is really working after your change.

### Legend

| Mark | Meaning |
|---|---|
| ✅ | Done + tested + verified |
| 🔄 | In progress (name the sandbox/branch in the row) |
| ⬜ | Not started, designed or documented |
| 💭 | Idea / discussion, not designed yet |
| 🐛 | Known bug — open, see Bug tracker |
| ⛔ | Deliberately out of scope (with reason) |

---

## 1. Download engine (hyprfetch-core)

| # | Task | Status | Tests / Proof |
|---|---|---|---|
| 1.1 | Segmented downloader: N parallel `Range` connections, `pwrite` to one shared fd | ✅ | `engine::tests::end_to_end_download_single_segment`, ws e2e |
| 1.2 | Segment planner `planner::split()` (last segment absorbs remainder) | ✅ | planner unit tests |
| 1.3 | File pre-allocation (`ftruncate`) before segmented write | ✅ | resume tests (byte-exact files) |
| 1.4 | Per-segment retry: transient errors, 6 attempts, exponential backoff 1→15 s, resume from last offset | ✅ | `is_retryable` / `backoff_delay` unit tests |
| 1.5 | Timeouts: `connect_timeout(10s)` + idle `read_timeout(30s)`, NO total request timeout | ✅ | documented regression — see README test section |
| 1.6 | **Servers without Range support** — accept `200 OK` whole-file body when segment starts at byte 0 (single-segment fallback), cap writes at segment end | ✅ (fixed in this audit) | `download_from_server_without_range_support` |
| 1.7 | Mid-session remote-change recheck: on every (re)start compare stored ETag/Last-Modified/size vs fresh probe; discard offsets on mismatch | ✅ (fixed in this audit) | `resume_after_remote_change_mid_session_resets_offsets` |
| 1.8 | Segment auto-tuning (split slow segments, reduce count on saturation) | ⬜ | — |
| 1.9 | Per-request extra headers passed to workers (auth/referer) | ✅ | create-task `headers` round-trip |
| 1.10 | Redirect handling: follow max 5, re-run SSRF check on final URL | ✅ | `HttpClient` unit tests |
| 1.11 | HTTP/2 explicit multi-connection handling (avoid single-TCP-window multiplexing trap) | ⬜ | — |
| 1.12 | HTTP/3 (QUIC) via `h3`/`quinn` | 💭 | — |

**Verify:** `cargo test -p hyprfetch-core`

## 2. Resume & integrity

| # | Task | Status | Tests / Proof |
|---|---|---|---|
| 2.1 | HEAD probe: Content-Length / Accept-Ranges / ETag / Last-Modified | ✅ | http_client tests |
| 2.2 | Per-segment offsets persisted to SQLite (debounced 500 ms) | ✅ | persistence tests |
| 2.3 | Startup resume pass: reload queued/downloading/paused, validate remote, restart from offsets; crash-leftover `downloading` normalized to `paused` | ✅ | 6 resume tests |
| 2.4 | Remote changed → offsets discarded, restart from byte 0 | ✅ | `resume_all_resets_when_etag_changed` |
| 2.5 | Local file missing/truncated vs stored total → offsets discarded | ✅ | `resume_all_restarts_stale_task_with_corrupt_local_file` |
| 2.6 | Probe failure at startup → task marked `error` (retryable via `POST /retry`), pass never aborts | ✅ | `resume_all_marks_error_when_probe_fails` |
| 2.7 | Checksum verification (hash download while streaming; expose via API) | ⬜ | — |
| 2.8 | `Content-Disposition` filename respect | ⬜ | — |

**Verify:** `cargo test -p hyprfetch-core resume`

## 3. QoS / bandwidth sharing (the differentiator)

| # | Task | Status | Tests / Proof |
|---|---|---|---|
| 3.1 | Engine-wide governor token bucket shared by all active tasks | ✅ | qos tests (shared budget) |
| 3.2 | Live retune via `PUT /api/qos` (atomic bucket swap) | ✅ | `set_qos_applies_to_engine_limiter_live` |
| 3.3 | Per-task `qos_override: force_off` bypass | ✅ | ws e2e `force_off_task_bypasses_qos` |
| 3.4 | QoS restored from settings at startup | ✅ | `qos_loaded_from_settings_on_construction` |
| 3.5 | Measured accuracy: 4.10 MB/s avg vs 4 MiB/s target over a full 1 GB download (±2.2%) | ✅ | README sandbox test data |
| 3.6 | "Auto" QoS mode: measure max bandwidth, cap at % of it | ⬜ | — |
| 3.7 | Kernel `tc` helper (fq_codel/htb) | ⛔ deferred — app-level limiter sufficient for v1 | — |

**Verify:** `cargo test -p hyprfetch-core qos && cargo test -p hyprfetch-api --test ws_events`

## 4. Concurrency & queue

| # | Task | Status | Tests / Proof |
|---|---|---|---|
| 4.1 | **Finished tasks reaped** from the engine's active map (was: leaked forever; `is_running` stayed true) | ✅ (fixed in this audit) | `finished_task_is_reaped_from_active_map` |
| 4.2 | **Queue pump enforcing `max_concurrent_tasks`**: queued tasks start oldest-first as slots free; 0/missing = unlimited; explicit user resumes bypass the cap | ✅ (fixed in this audit) | `queue_pump_respects_max_concurrent_tasks`, `queue_pump_unlimited_drains_queue` |
| 4.3 | Double-spawn guard: `spawn_coordinator` is check-and-insert atomic; pump pass filters already-started ids | ✅ (fixed in this audit) | covered by pump tests (was causing spurious `removed` states) |
| 4.4 | `max_connections` global cap on concurrent segment connections | ⬜ (setting seeded, not enforced) | — |
| 4.5 | Pause-all / resume-all endpoints | ⬜ | — |
| 4.6 | Queue reordering API (drag/move priority) | 💭 | — |

**Verify:** `cargo test -p hyprfetch-core queue_pump`

## 5. HTTP API (hyprfetch-api)

| # | Task | Status | Tests / Proof |
|---|---|---|---|
| 5.1 | `GET /healthz`, list/create/get/delete tasks, pause/resume/cancel, QoS get/put, settings get/patch | ✅ | routes tests (16) |
| 5.2 | **`POST /api/tasks/:id/retry`** (was documented-planned) — Error→Queued, error cleared, pump kicked | ✅ (added in this audit) | `retry_moves_errored_task_to_queued_and_clears_error`, `retry_on_queued_task_returns_409`, `retry_on_missing_task_returns_404` + live smoke |
| 5.3 | **`DELETE /api/tasks/:id?delete_file=true`** removes the (partial) file from disk | ✅ (added in this audit) | `delete_with_delete_file_removes_file_from_disk`, `delete_without_param_keeps_file_on_disk` + live smoke |
| 5.4 | Bulk create (≤100 URLs) with per-request `save_dir`/`segments`/`headers`/`qos_override` | ✅ | create tests |
| 5.5 | Structured errors with stable codes | ✅ | error tests |
| 5.6 | Task filtering: `?state=active|completed|all` | ✅ | list tests |
| 5.7 | Pagination for large task lists | ⬜ | — |
| 5.8 | OpenAPI spec generation | 💭 | — |

**Verify:** `cargo test -p hyprfetch-api`

## 6. WebSocket (`/ws`)

| # | Task | Status | Tests / Proof |
|---|---|---|---|
| 6.1 | Event fan-out: `task:progress` (500 ms throttle), `task:state`, `global:speed` (1 s + idle zero-event) | ✅ | ws e2e over real TCP |
| 6.2 | Slow-client policy: skip missed events (progress is absolute) | ✅ | by design (broadcast lag) |
| 6.3 | Per-event type subscriptions / message filtering | 💭 | — |

**Verify:** `cargo test -p hyprfetch-api --test ws_events`

## 7. Web UI (Svelte SPA, embedded via rust-embed)

| # | Task | Status | Tests / Proof |
|---|---|---|---|
| 7.1 | Active list with live progress bars, add-task modal (multi-URL, save dir, segments), QoS toggle | ✅ | `ui_index_is_served`, `ui_assets_are_served_and_api_404s_are_not_spa` |
| 7.2 | **Retry button on errored tasks** (Active + Finished lists) | ✅ (added in this audit) | `dist/` rebuilt; live smoke |
| 7.3 | **Remove-with-file button (✕) in Finished list** now calls `?delete_file=true` | ✅ (added in this audit) | `dist/` rebuilt |
| 7.4 | Settings page (paths, segments, QoS defaults) | ⬜ | — |
| 7.5 | History page with retention | ⬜ | — |
| 7.6 | Stats page (60 s aggregate speed line, totals) | ⬜ | — |
| 7.7 | Per-segment mini-bars (expandable) | ⬜ | — |
| 7.8 | Auth-aware UI (token prompt when daemon is non-loopback) | ⬜ | — |
| 7.9 | Pause-all / resume-all buttons | ⬜ | — |

**Verify:** `cd crates/hyprfetch-api/ui && npm run build` then serve + click through.

## 8. Security

| # | Task | Status | Tests / Proof |
|---|---|---|---|
| 8.1 | SSRF policy: block private/loopback/link-local/CGNAT (v4+v6), DNS-resolved | ✅ | ssrf tests |
| 8.2 | `--allow-private` CLI override (default protected) | ✅ | engine tests use it |
| 8.3 | **Bearer-token auth on non-loopback binds** (`Authorization: Bearer` or `?access_token=`; `401` + `WWW-Authenticate`; `/healthz` + SPA stay open) — was documented in api.md but NOT implemented | ✅ (added in this audit) | 7 auth tests + live smoke (401/200/query-param) |
| 8.4 | Token auto-generation on first run, stored `~/.config/hyprfetch/token` (0600); resolution order CLI > env > config > settings > file | ✅ (added in this audit) | live smoke; `doctor` shows status |
| 8.5 | **`ssrf_block_private` setting honored** at startup (was seeded-dead) | ✅ (added in this audit) | live smoke |
| 8.6 | Path traversal: filenames sanitized, prefixed with download dir | ✅ | create tests |
| 8.7 | Loopback binds never require auth | ✅ | live smoke |
| 8.8 | Global rate limiting / brute-force lockout on the token endpoint | ⬜ | — |
| 8.9 | `cargo audit` clean in CI | ✅ | ci.yml job |

**Verify:** `cargo test -p hyprfetch-api auth` + bind `0.0.0.0` and curl without/with token.

## 9. Persistence (hyprfetch-db)

| # | Task | Status | Tests / Proof |
|---|---|---|---|
| 9.1 | SQLite (bundled, WAL), migrations runner, tasks/segments/settings/events repos | ✅ | repo tests |
| 9.2 | Cascade delete of segments with task | ✅ | `delete_cascades_to_segments` |
| 9.3 | Event log table (audit: task.created/removed/paused…) | ✅ | events tests |
| 9.4 | Retention policy for events / finished tasks | ⬜ | — |
| 9.5 | Schema-version reporting in `doctor` | ⬜ | — |

**Verify:** `cargo test -p hyprfetch-db`

## 10. CLI & configuration

| # | Task | Status | Tests / Proof |
|---|---|---|---|
| 10.1 | `serve` (bind/db-path/download-dir/segments/allow-private) + `doctor` | ✅ | live smoke |
| 10.2 | `HYPRFETCH_*` env vars for every flag | ✅ | clap `env` attrs |
| 10.3 | **Config file** `~/.config/hyprfetch/config.toml` (`bind`, `db_path`, `download_dir`, `segments`, `allow_private`, `api_token`), `--config` override, CLI > env > file > default — was documented but NOT implemented | ✅ (added in this audit) | live smoke (config loaded, settings applied) |
| 10.4 | **`user_agent` setting honored** for outgoing requests (was seeded-dead) | ✅ (added in this audit) | `HttpClient::with_user_agent` + engine wiring |
| 10.5 | `max_connections` / `protocol_pref` enforcement | ⬜ | see 4.4 / 1.11–1.12 |
| 10.6 | `bind` setting actually driving the default bind (currently informational only) | ⬜ | — |
| 10.7 | Shell completions (bash/zsh/fish) | ⬜ | — |
| 10.8 | systemd user unit shipped in packages | ⬜ | — |

**Verify:** `hyprfetch --help`; `hyprfetch serve --config <file>` with a sample file.

## 11. Packaging & release (CI)

| # | Task | Status | Tests / Proof |
|---|---|---|---|
| 11.1 | Release tarballs: x86_64-gnu / aarch64-gnu / x86_64-musl + sha256 | ✅ | v0.1.1 + v0.2.0 releases |
| 11.2 | aarch64 cross-link fix (AArch64 GNU toolchain wired into cargo) | ✅ | v0.1.1 release run green |
| 11.3 | `.deb` (cargo-deb, amd64) attached to releases | ✅ | v0.2.0 asset + dpkg-deb inspection |
| 11.4 | `.rpm` (rpmbuild from spec, x86_64) attached to releases | ✅ | v0.2.0 asset |
| 11.5 | `PKGBUILD` (Arch, generated per release with pinned version+sha256) attached | ✅ | v0.2.0 asset; sha256 verified byte-exact |
| 11.10 | Desktop entry (`packaging/desktop/hyprfetch.desktop`) shipped in tarball + installed by PKGBUILD/.deb/.rpm at `/usr/share/applications/` | ✅ | 2026-09-29: file in tarball step of release.yml; PKGBUILD/rpm-spec/deb-assets install it; see CHANGELOG |
| 11.11 | Default download dir switched to `~/Desktop` (was `~/Downloads`), overridable via `save_dir`/setting/`--download-dir` | ✅ | 2026-09-29: `routes.rs` fallback `+ "/Desktop"`; 111 unit tests green; docs updated |
| 11.6 | arm64 `.deb` (aarch64 Debian package) | ⬜ | — |
| 11.7 | AUR submission (`hyprfetch-bin`) from the release PKGBUILD | ⬜ | — |
| 11.8 | Windows / macOS builds | ⛔ out of scope for now (Linux-first project) | — |
| 11.9 | systemd unit + postinst scripts in .deb/.rpm | ⬜ | — |

**Verify:** push a tag; check the release assets; or run `.github/workflows/release.yml` via `workflow_dispatch`.

## 12. Docs

| # | Task | Status | Proof |
|---|---|---|---|
| 12.1 | `docs/architecture.md` (what the system is) | ✅ | — |
| 12.2 | `docs/design.md` (why: language choice, QoS, gotchas, roadmap) | ✅ | — |
| 12.3 | `docs/api.md` (endpoints, auth, WS contract, settings-enforcement table) | ✅ (updated this audit: retry, delete_file, auth reality, enforcement table) | — |
| 12.4 | `docs/install.md` (deb/rpm/PKGBUILD/tarball/source) | ✅ | — |
| 12.5 | `docs/development.md` (dev setup, git workflow, release checklist) | ✅ | — |
| 12.6 | README sandbox test data + known-gaps ledger | ✅ (updated this audit) | — |
| 12.7 | `docs/threat-model.md` (expand of architecture §Threat model) | ⬜ | — |
| 12.8 | Screenshots / GIF of the UI in README | ⬜ | — |

## 13. Testing / QA

| # | Task | Status | Proof |
|---|---|---|---|
| 13.1 | Unit + integration suite (engine, api, db, ws e2e) | ✅ | **111 passing** (110 before resume-race regression test) |
| 13.2 | CI: fmt + clippy `-D warnings` + test + audit + MSRV check | ✅ | ci.yml |
| 13.3 | Real-network sandbox tests (1 GB / 5 GB thinkbroadband, pause/resume, QoS accuracy, RSS) | ✅ | README test section |
| 13.4 | Load test: 50+ concurrent tasks / fd-limit behavior | ⬜ | — |
| 13.5 | Flaky-network simulation (packet loss, mid-body EOF storm) | ⬜ | — |
| 13.6 | Memory regression test (idle RSS < 10 MB in CI) | ⬜ | — |
| 13.7 | UI e2e (Playwright against the served SPA) | ⬜ | — |

## 14. Infrastructure (.github)

| # | Task | Status | Proof |
|---|---|---|---|
| 14.1 | Workflows: ci.yml, release.yml, stale.yml | ✅ | runs green |
| 14.2 | Issue templates (bug/feature), PR template | ✅ | — |
| 14.3 | dependabot.yml (cargo + github-actions, weekly) | ✅ | v0.2.0-era runs |
| 14.4 | Multi-sandbox protocol: every commit updates CHANGELOG.md + worklog.md | ✅ | worklog.md |
| 14.5 | Release-notes automation (generate_release_notes) | ✅ | v0.2.0 release |
| 14.6 | Nightly build workflow (main → artifact) | ⬜ | — |
| 14.7 | Benchmark workflow (engine throughput regression) | 💭 | — |
| 14.8 | **Dependency sync 2026-09-28**: merged all 10 Dependabot branches into `main` (cargo: thiserror 2.0.21, toml 1.1.6, tokio-tungstenite 0.24.0, rusqlite 0.40.2, governor 0.10.4; actions: checkout@v7, upload-artifact@v7, download-artifact@v8, stale@v11, action-gh-release@v3); repaired governor-merge Cargo.lock duplication; branches deleted, PRs #10–#19 closed | ✅ | fmt/clippy clean, **110/110 tests**, live smoke (config/WAL/download/doctor), merge commits `36be434`–`a79d93d` |
| 14.9 | **Dependency sync round 2**: merged axum 0.7.9→0.8.9 (route `{id}` syntax + WS `Utf8Bytes` code fixes) and tokio-tungstenite 0.24→0.30.0; branches deleted, PRs #20/#21 closed — remote has ONLY `main` | ✅ | fmt/clippy clean, **111/111 tests**, E2E **30/30** incl. 1 GB download, merges `e761928`/`23e5b93` |

---

## 15. Run modes & self-update (v0.3.1 — owner request, APPLIED 2026-09-29)

Design sketch: `docs/feature-research.md` → "Roadmap". **All rows applied and
verified** — E2E suite `scripts/e2e_full_test.py` (73/73) + workspace tests
117/117 + clippy clean.

| # | Task | Status | Tests / Proof |
|---|---|---|---|
| 15.1 | Dev mode: `hyprfetch dev` / `serve --mode dev` — debug tracing, pretty console, auto-open UI | ✅ | binary `--help`; E2E UI phase; dev logger unit path |
| 15.2 | Prod daemon mode: `daemon start` — detached, PID file + rotating logs under state dir (`HYPRFETCH_STATE_DIR` > XDG) | ✅ | E2E daemon phase: start/status/logs/restart/stop all green |
| 15.3 | Lifecycle subcommands: `hyprfetch logs [-f] [-n N]` / `daemon status` / `stop` / `restart` (pm2-style UX) | ✅ | E2E daemon phase 7/7 |
| 15.4 | Updater check: GitHub release query (PAT, private repo) + "new version" in UI settings card + `daemon status` | ✅ | E2E mock-GitHub phase: check + cache + `/api/server` flag; live check against real private repo (PAT) |
| 15.5 | Updater apply: binary mode (tarball → sha256 verify → atomic swap → restart) and source mode (`--from-git`: pull → build → swap), with drain-pause → restart → auto-resume | ✅ | E2E: apply swaps on-disk binary, sha256 verified, `.old` backup; restart spawns NEW process, replacement healthy; CLI `update --yes` full path |
| 15.6 | `GET /api/server` (version, uptime, active tasks, ws clients, update flag) | ✅ | E2E UI phase + ws phase |
| 15.7 | Sleep mode: `--exit-when-idle <min>` — graceful exit after fully-idle budget | ✅ | E2E exit-when-idle phase (3s budget → clean exit code 0) |
| 15.8 | `--workers <n>` runtime sizing, default 2 (IO-bound, minimal RAM) | ✅ | build + serve smoke; RSS phase |
| 15.9 | Web UI restyle: Tailwind + DaisyUI (`dim` theme), ~18 KiB gzipped total bundle | ✅ | E2E UI phase: dim theme + daisy css served; npm build |
| 15.10 | Restart hand-off race fixed (bind retry window 15 s) | ✅ | E2E restart phase: replacement healthy on same port |

**Measured (E2E resource phase):** idle RSS 8.0 MB, peak 9.4 MB during a
segmented download, no leak after completion.

---

## 16. Multi-page UI + category folders + zero-config updates (v0.3.1 re-cut — owner request, APPLIED 2026-09-29)

| ID | Feature | Status | Proof |
|---|---|---|---|
| 16.1 | Multi-page web UI: hash router + Dashboard / Tasks / Settings / Updates pages | ✅ | E2E UI phase (index + hashed assets + SPA deep-link fallback); pages build in one tiny bundle |
| 16.2 | Dashboard: live speed, per-state counters, active downloads, recent finished, folder overview | ✅ | manual smoke + E2E server endpoints feeding it |
| 16.3 | Tasks page: full table (filters, search, progress, save path, per-task controls) | ✅ | E2E task lifecycle phases |
| 16.4 | Category model: 7 folders (video/pictures/music/compress/documents/apps/other), extension map in `hyprfetch-core/src/categories.rs` | ✅ | unit tests `categories::*` (13 cases) |
| 16.5 | Folders auto-created at startup, on settings change, and at task creation | ✅ | E2E "all category folders auto-created on disk" + "category override dir auto-created" |
| 16.6 | Auto-sort by file type (default on), explicit `category` per task, `none` = base dir | ✅ | E2E: zip→compress/, music category, none→base |
| 16.7 | Direct save (`save_dir`) wins over categorization | ✅ | E2E "direct save beats category" |
| 16.8 | Per-category overrides `category_dir_<name>` + Settings page editors | ✅ | E2E override dir created; unit test `dir_for_category_prefers_override` |
| 16.9 | `GET /api/categories` endpoint; `category` field on task DTOs | ✅ | E2E categories phase |
| 16.10 | Default base dir `~/Desktop` → `~/Downloads` | ✅ | docs + E2E base check |
| 16.11 | Zero-config updater for clone installs: PAT parsed from clone origin URL (`~/HyprFetch` + common paths + `[update] source_dir`) | ✅ | unit tests `pat_parsing_from_git_urls`; live `update --check` authenticated with only a clone in `$HOME` |
| 16.12 | GitHub token settable from UI settings, masked on `GET /api/settings` (`<key>_set`) | ✅ | unit test `settings_mask_github_token`; E2E mask check |
| 16.13 | E2E suite rebuilt post-sandbox-recycle: 43/43 full + 4/4 large (1 GiB sha256-exact) | ✅ | `/home/z/my-project/scripts/e2e_full_test.py`, `e2e_large_test.py` |

**Measured:** E2E full suite 43/43 green; 1 GiB download byte-exact with
mid-flight pause/resume (16 MB/s loopback incl. per-chunk pattern
generation); cargo test 130 green; clippy `-D warnings` clean.

## 17. Private-repo self-update without a token (v0.3.2 — owner request, APPLIED 2026-09-29)

Owner report: on their Arch box the repo is linked via **SSH** (`git clone
git@github.com:…`), so no PAT exists anywhere — `hyprfetch update --check`
printed "unauthenticated (public repos only)" and gave up. SSH keys cannot
call the GitHub REST API, but they CAN run git — so the updater gained a
git tier.

| ID | Item | Status | Proof |
|---|---|---|---|
| 17.1 | Core git tier: `git ls-remote --tags` version discovery (`parse_version_tags`, `latest_version_tag`, `ls_remote_latest_tag`) with non-interactive env (batch-mode SSH, no prompts) | ✅ | unit tests `version_tag_parsing`, `latest_tag_selection`, `ls_remote_and_shallow_clone_on_local_repo` (real subprocess over `file://` repo) |
| 17.2 | Git-tier check: `git_url_candidates` (config `git_url` → clone origin → derived `git@github.com:<repo>.git` → anonymous HTTPS) + `check_via_git(_sync)` returning `GitCheck{via_url, tag}` | ✅ | unit test `git_candidate_ordering`; E2E scenario C "git tier via file:// remote" |
| 17.3 | Git-tier install: `clone_tag_shallow` + `find_cargo` (PATH → `~/.cargo/bin`) + `cargo_build_release` + `build_from_tag` → atomic swap → daemon restart | ✅ | E2E scenario C4: `installed 9.9.9`, swapped binary reports 9.9.9, `hyprfetch.old` kept, temp dir cleaned |
| 17.4 | Token chain widened + labelled: `GH_TOKEN`, `gh auth token`, `git credential fill` join the chain; `token_source` shown in CLI (`authenticated (env)`…) and doctor | ✅ | E2E scenario B label check; doctor scenario D |
| 17.5 | Clone-origin discovery returns BOTH PAT and origin URL (`detect_git_from_source_clones`) so SSH clones feed the git tier with zero config | ✅ | E2E scenario D: doctor shows `update git = git@github.com:Local-DE-Coach/HyprFetch.git` |
| 17.6 | `update` CLI loads `[update]` config section + new `--config` flag | ✅ | E2E scenario C uses config-file `git_url` |
| 17.7 | API `GET /api/update/check` git-tier fallback, `via_git` in payload; `POST /api/update/apply` returns 400 with CLI guidance when `via_git` | ✅ | build + existing E2E "update check endpoint graceful" still green |
| 17.8 | No-auth UX: prints what to do (env / `gh auth login` / SSH clone / config) instead of a dead end | ✅ | E2E scenario A hints checks |
| 17.9 | Docs: README Self-update (two tiers), install.md "Keeping it updated" (+ `git_url` example), api.md (via_git + 400) | ✅ | docs diff |
| 17.10 | New E2E `scripts/e2e_update_tiers.sh` — 4 scenarios / 20 checks, all green | ✅ | RESULT: 0 failed check(s) |

**Measured:** cargo fmt + clippy clean; 134 workspace tests green; main
downloader E2E 43/43 green; updater E2E 20/20 green (incl. a full
clone→build→swap cycle against a local git remote and the PAT tier against
the real v0.3.1 GitHub release).

## 18. Self-hosted fast update channel on istias.tech (v0.3.3 — owner request, APPLIED 2026-09-29)

Owner request: "use my server (istias.tech / 138.197.73.65) so
`hyprfetch update --check` is fast — and deploy each release there in a NEW
directory so the existing webUI on that server keeps working." Result: the
updater gained a tier 0 — a self-hosted HTTPS manifest mirror that answers
with one fast GET (no GitHub API, no rate limits, no tokens).

| ID | Item | Status | Proof |
|---|---|---|---|
| 18.1 | Core channel tier: `ChannelManifest`/`ChannelAsset` (serde), `manifest_url`, `pick_channel_asset` over `target_candidates()`, `effective_channel()` (empty = disabled) | ✅ | unit tests `manifest_url_join`, `channel_effective_url`, `manifest_parses_and_picks_host_asset`, `manifest_requires_version_and_assets` |
| 18.2 | `check_via_channel` — GET `latest.json`, compare `version`, map the host asset into the shared `UpdateCheck` (via_channel + channel fields) | ✅ | E2E scenario B: "checked via update channel", version + asset lines |
| 18.3 | `apply_channel` — download manifest-listed tarball → sha256 vs manifest → `extract_binary` → `swap_binary` (atomic, `.old` kept) | ✅ | E2E scenario C: installed 9.9.9, swapped binary reports 9.9.9; scenario D: tampered sha256 refused, binary untouched |
| 18.4 | CLI three-tier chain (channel → API → git) with per-tier labels + `--channel` flag; `--check` fast-path hint | ✅ | E2E scenarios B/C/F: labels + `update channel unreachable (…) — trying GitHub…` |
| 18.5 | Config + env: `[update] channel` (default = `https://istias.tech/hyprfetch/updates/`, `""` disables), `HYPRFETCH_UPDATE_CHANNEL`; precedence flag > env > config > default; doctor channel line | ✅ | E2E scenario G ("" disables silently); doctor prints `update channel = …` |
| 18.6 | API: `GET /api/update/check` channel-first (`"via_channel": true` + `"channel"` in payload); `POST /api/update/apply` installs from the mirror when cached check was channel | ✅ | E2E scenario H payload checks |
| 18.7 | UI: Updates card shows `via update channel` badge + updated copy (channel-first wording) | ✅ | `ui/src/pages/Updates.svelte` diff; dist rebuilt + committed |
| 18.8 | Release workflow `deploy-update-channel` job: generate `latest.json` (per-target url/sha256/size from built archives), scp archives into `<DEPLOY_PATH>/updates/<version>/`, atomic top-level manifest swap, post-deploy public-URL verification; skips with notice when `DEPLOY_SSH_KEY` secret absent | ✅ | YAML validated; secrets table + deploy-key guide in docs/update-channel.md |
| 18.9 | `docs/update-channel.md`: manifest schema, one-time nginx `location /hyprfetch/` setup (new dir, existing site untouched), Cloudflare notes, secrets, client overrides, verification | ✅ | docs diff; README/install.md/api.md synced to three-tier wording |
| 18.10 | New E2E `scripts/e2e_update_channel.sh` — 8 scenarios / 28 checks all green; `e2e_update_tiers.sh` made hermetic for channel-first (channel disabled per scenario, dead API base for git-tier install) — 20/20 green again | ✅ | RESULT: 0 failed check(s) in both scripts |

**Measured:** cargo fmt + clippy clean; **138** workspace tests green;
main downloader E2E **43/43** green; channel E2E **28/28**; updater tiers
E2E **20/20**. Side finding: the GitHub repo is **public** now
(unauthenticated `ls-remote` + API answers), so the API/git tiers succeed
against the real repo — scenarios that need the dead-end UX pin
`--repo fake/nonexistent-repo` + `--channel ""` to stay hermetic.

## 19. Server-only updater v0.4.0 + istias.tech docs pages (owner request, APPLIED 2026-09-29)

Owner request: (1) remove GitHub from the update flow in ANY mode — only the
own server; (2) GitHub Action ships every release to the server in a new
path; (3) new pages in **Local-DE-Coach/Docs** (the repo that OWNS the
istias.tech server webUI) at `/hyprfetch` + `/hyprfetch/updates`; (4) show
the latest version on those pages; (5) clean asset names; (6) richer release
notes; keep the 500 MB-RAM VPS budget.

| ID | Item | Status | Proof |
|---|---|---|---|
| 19.1 | Updater stripped to ONE source: channel-only `update.rs` (API tier, git tier, token discovery, clone scan, `--from-git` all deleted; `UpdateConfig` = `{channel_url}`); unreachable → clear error + updates-page pointer, no fallback | ✅ | 9 core unit tests; E2E scenario 5 (asserts NO GitHub attempt in output) |
| 19.2 | CLI: `--repo/--token/--from-git/--source-dir` removed; `--channel ""` disables with message; doctor prints channel; legacy `[update]` keys accepted-but-ignored | ✅ | helpers tests (precedence, disable, legacy TOML parse); E2E scenario 6 |
| 19.3 | API: `/api/update/check` channel-only (error payload adds `updates_page`; legacy `via_*` fields gone); `/api/update/apply` channel-only | ✅ | E2E scenario 7 payload checks |
| 19.4 | UI: Settings GitHub-token card → **Update channel** card; Updates card copy; dist rebuilt | ✅ | `ui/src/pages/{Updates,Settings}.svelte` diff + dist commit |
| 19.5 | Clean asset names: `hyprfetch-<ver>-linux-{x64,arm64,musl-x64}.tar.gz`; manifest maps clean names → target triples; PKGBUILD + rpm spec repinned | ✅ | release.yml diff; packaging template diffs; docs tables |
| 19.6 | Release notes auto-generated per release: CHANGELOG `## [X.Y.Z]` section + update/install commands + install table + downloads table + sha256 verify + page links (python3 assembly, injection-safe) | ✅ | `scripts/simulate_release_notes.sh` green (extracts the run block and runs it exactly like the runner) |
| 19.7 | Docs repo: `/hyprfetch` product page + `/hyprfetch/updates` page (static prerender, client-side manifest fetch, graceful "channel warming up" state) | ✅ | build green (18 routes, both pages `○`); LIVE: `/hyprfetch` 200, `/hyprfetch/updates` 200 |
| 19.8 | Docs deploy.yml self-heals the update channel: creates `/var/www/istias.tech/hyprfetch/updates` (world-readable) + nginx exact-match page proxy + `^~` static alias (CORS, max-age=60); also fixed the Sep-19 deploy breaker (raw `<60` in test-results JSX) | ✅ | Docs deploy run #2 success after fix; live checks above |
| 19.9 | New E2E `scripts/e2e_update_channel.sh` COMMITTED — 8 scenarios / 23 checks (check, up-to-date, full install swap+backup, tampered-sha refusal, unreachable w/o GitHub, disabled, API payload, malformed manifest) | ✅ | RESULT: PASS 23 / FAIL 0 |
| 19.10 | Docs synced: README Self-update (one source), update-channel.md rewritten (automated routing; only DEPLOY_SSH_KEY left for the owner), install.md, api.md, CHANGELOG 0.4.0 | ✅ | docs diffs |

**Measured:** 133 workspace unit tests green (`CARGO_INCREMENTAL=0`);
channel E2E 23/23; Docs portal build green; live pages verified. Note: the
old `e2e_update_tiers.sh` and token-tier scenarios are OBSOLETE — the tiers
they exercised no longer exist (superseded by 19.1).

## Bug tracker (open)

| ID | Bug | Status | Notes |
|---|---|---|---|
| B1 | ~~Finished tasks never reaped from engine map~~ | ✅ fixed (4.1) | audit 2026-09-28 |
| B2 | ~~Downloads from non-Range servers always failed~~ | ✅ fixed (1.6) | audit 2026-09-28 |
| B3 | ~~Double-spawn race in queue pump → spurious `removed` states~~ | ✅ fixed (4.3) | audit 2026-09-28 |
| B4 | Remote mutated *mid-download* (no validator change timing) | 🐛 open | inherent HTTP limitation; mitigations: ETag/If-Range honest servers; candidate: post-download length+validator re-probe |
| B5 | 200-response acceptance when server starts ignoring Range mid-task (multi-segment) errors the task instead of degrading gracefully | 🐛 open | rare; retry re-probes and single-segments it |
| B6 | ~~Resume immediately after pause strands task in `downloading` forever (route pre-flipped state + engine.start rejected it + error swallowed)~~ | ✅ fixed | engine accepts stranded `downloading` rows; resume route retries then rolls back to paused + 409; regression test `resume_immediately_after_pause_does_not_strand_task` — found by live E2E 2026-09-28 |

## Backlog (future ideas, not designed)

- Browser-extension / share-target integration
- yt-dlp sidecar for media extraction
- Metalink / multi-source downloads
- Torrent support — ⛔ explicitly out of scope (see architecture.md non-goals)
- rsync/S3 destinations as download targets
- Per-task bandwidth shares (weighted fair queueing under one QoS cap)
- Import/export task list (JSON)
- Wildcard/mirror URL sets (aria2-style multi-mirror)
- Proxy support (HTTP/SOCKS5 per task or global)
- Bandwidth scheduler (time-of-day QoS profiles)
