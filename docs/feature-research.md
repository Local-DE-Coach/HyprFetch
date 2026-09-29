# Feature research — Linux download managers vs HyprFetch

> **Source note (2026-09-29).** The owner uploaded
> `Linux-Download-Manager-Research.md` and `Feature-Tree-Diagram.png` as the
> basis for this document, but the upload did not reach the build sandbox
> (the file never arrived on disk). This checklist was therefore rebuilt from
> the standard Linux download-manager research dimensions (aria2, Motrix,
> Persepolis, XDM, uGet, DownThemAll, Free Download Manager) and verified
> **line-by-line against the actual HyprFetch code, tests and E2E runs**
> (commit `ddfdd41`, v0.2.0). Every ✅ names its proof. Re-upload the original
> file and any wording or missing rows can be merged in verbatim without
> touching the status marks.

**Legend:** ✅ shipped & verified · 🟨 partial (read the note) · ⬜ not implemented (listed unchanged, no design yet)

## Snapshot

| Area | ✅ | 🟨 | ⬜ |
|---|---|---|---|
| 1. Download core & integrity | 6 | — | 1 |
| 2. Protocols & sources | 2 | — | 5 |
| 3. Speed & bandwidth | 2 | 1 | 2 |
| 4. Queue & task management | 5 | — | 4 |
| 5. Web UI & API | 5 | — | 3 |
| 6. Security & privacy | 5 | — | 2 |
| 7. CLI & configuration | 5 | — | — |
| 8. Persistence & state | 3 | — | 1 |
| 9. Packaging & distribution | 4 | — | 2 |
| 10. Desktop & browser integration | 2 | — | 3 |
| 11. Automation & post-processing | — | — | 4 |
| 12. Ops: run modes, logging, updates | 1 | 1 | 2 |

---

## 1. Download core & integrity

| Feature | Status | Proof in HyprFetch |
|---|---|---|
| Segmented / multi-connection downloads (1–32 per task) | ✅ | `hyprfetch-core` engine; `--segments` flag; E2E "create local task (4 segments)" |
| Byte-exact resume after pause (HTTP Range) | ✅ | E2E "50MB after resume byte-exact (sha256)"; SQLite-persisted offsets |
| Auto-resume interrupted tasks at startup | ✅ | engine startup re-probe; E2E + worklog 2026-09-28 session |
| Remote-file change detection (ETag / Last-Modified) → clean restart | ✅ | engine probe logic; local Range server sends fixed ETag |
| Redirect following with per-hop re-validation | ✅ | `ssrf.rs` redirect-chain validator (each hop re-checked) |
| Checksum verification after download (sha256/md5 file or sidecar) | ⬜ | — |
| Non-Range servers still work (single-connection fallback) | ✅ | fixed in v0.1.x; CHANGELOG "Downloads from servers without Range support" |

## 2. Protocols & sources

| Feature | Status | Proof |
|---|---|---|
| HTTP / HTTPS | ✅ | engine (reqwest); all E2E downloads |
| Multi-URL batch create in one request | ✅ | `POST /api/tasks` accepts up to 100 URLs |
| FTP / FTPS | ⬜ | rejected as invalid scheme by design today |
| SFTP / SSH | ⬜ | — |
| BitTorrent / magnet links | ⬜ | — |
| Metalink (`.meta4`) | ⬜ | — |
| Video-site extraction (youtube-dl style) | ⬜ | — |

## 3. Speed & bandwidth

| Feature | Status | Proof |
|---|---|---|
| Engine-wide QoS cap (token-bucket) | ✅ | governor limiter; E2E "10MB under 1MiB/s took ≥6s"; `PUT /api/qos` |
| Live speed reporting (per task + global) | ✅ | WS `global:speed` frames; E2E WS check |
| Per-task speed limit | ⬜ | — |
| Scheduled speed (day/night profiles) | ⬜ | — |

## 4. Queue & task management

| Feature | Status | Proof |
|---|---|---|
| Task state machine queued→downloading→paused→complete→error | ✅ | 16 db tests + API tests |
| Concurrency limit / queue pump | ✅ | `max_concurrent_tasks` (default 3, 0 = unlimited) |
| Pause / resume / retry / delete (with or without file) | ✅ | E2E "retry on non-errored → 409", "DELETE ?delete_file=true" |
| Resume-after-pause race safety (no stranded tasks; 409 on failure) | ✅ | regression test `resume_immediately_after_pause_does_not_strand_task` |
| Automatic queueing of batches (max-100 guard) | ✅ | routes.rs create_task |
| Scheduling (start at time / periodic) | ⬜ | — |
| Priorities & reordering | ⬜ | — |
| Categories / tags / auto-sort into folders | ⬜ | — |
| Duplicate-URL detection | ⬜ | — |

## 5. Web UI & API

| Feature | Status | Proof |
|---|---|---|
| Embedded web UI, zero external server | ✅ | Svelte SPA via rust-embed; E2E asset checks |
| Live progress over WebSocket (idle-silent) | ✅ | E2E "WS live events" + "idle: 0 frames" |
| REST API (tasks, settings, qos, healthz) | ✅ | `docs/api.md`; E2E path coverage |
| UI actions: retry / pause / resume / delete | ✅ | bundle contains Retry; E2E |
| Settings editable in-app (download dir, segments…) | ✅ | `GET/PUT /api/settings`; E2E |
| UI login page / multi-user accounts | ⬜ | API token covers non-loopback only |
| Speed graph / statistics dashboard | ⬜ | — |
| Import / export task list | ⬜ | — |

## 6. Security & privacy

| Feature | Status | Proof |
|---|---|---|
| SSRF protection: private ranges, loopback, link-local, bad schemes | ✅ | `ssrf.rs`; E2E "bad scheme → 400" |
| Opt-out flag for private sources | ✅ | `--allow-private` / `HYPRFETCH_ALLOW_PRIVATE` |
| Loopback-only default bind | ✅ | E2E + `--bind` docs |
| API token required for non-loopback binds | ✅ | `docs/api.md`; startup log "loopback bind: API token not required" |
| No telemetry / phone-home | ✅ | design.md; no outbound calls beyond user downloads |
| Per-request custom HTTP headers (auth, referer…) | ✅ | `extra_headers` in engine + API |
| Encrypted / hashed token storage at rest | ⬜ | settings stored plain in SQLite |
| Fine-grained UI permissions | ⬜ | — |

## 7. CLI & configuration

| Feature | Status | Proof |
|---|---|---|
| `serve` / `doctor` subcommands, `--version` | ✅ | release binary; E2E server start |
| Full flag set with `HYPRFETCH_*` env overrides | ✅ | `--help`; precedence CLI > env > config > default |
| TOML config file (`~/.config/hyprfetch/config.toml`) | ✅ | parser + smoke test (toml 1.1) |
| Configurable default download dir (default `~/Desktop`) | ✅ | v0.2.0 re-release change; docs/install.md |
| Shell-friendly exit codes & structured errors | ✅ | API error envelope `{error:{code,message}}` |

## 8. Persistence & state

| Feature | Status | Proof |
|---|---|---|
| SQLite (WAL) with bundled build — no system dep | ✅ | rusqlite bundled; `doctor` pragma check |
| Settings repository | ✅ | `SettingsRepo`; seeded via migration 002 |
| Crash-safe resume (byte offsets persisted) | ✅ | E2E pause/resume + restart flows |
| Export / backup of task DB | ⬜ | — |

## 9. Packaging & distribution

| Feature | Status | Proof |
|---|---|---|
| Release tarballs: x86_64 gnu/musl + aarch64 (+ sha256) | ✅ | v0.2.0 release assets |
| .deb (amd64) + .rpm (x86_64) | ✅ | v0.2.0 release assets |
| Arch PKGBUILD generated per release (pinned version+sha) | ✅ | v0.2.0 `PKGBUILD` asset; sha256 verified |
| Desktop entry in every packaging channel | ✅ | `packaging/desktop/hyprfetch.desktop` → `/usr/share/applications/` |
| arm64 .deb | ⬜ | backlog 11.6 |
| Windows / macOS builds | ⬜ | out of scope (Linux-first) |

## 10. Desktop & browser integration

| Feature | Status | Proof |
|---|---|---|
| Desktop menu entry (launch `hyprfetch serve`) | ✅ | v0.2.0 desktop entry |
| `xdg-open` the web UI from CLI/docs | ✅ | optdepends + install guide |
| Browser extension / take over browser downloads | ⬜ | — |
| Clipboard URL monitoring | ⬜ | — |
| System tray icon / desktop notifications | ⬜ | — |

## 11. Automation & post-processing

| Feature | Status | Proof |
|---|---|---|
| Run command after download | ⬜ | — |
| Desktop notification on completion | ⬜ | — |
| Auto-retry with backoff on transient errors (manual retry exists) | ⬜ | manual `POST /tasks/:id/retry` only |
| Watch folder / poll a URL list file | ⬜ | — |

## 12. Ops: run modes, logging, updates

| Feature | Status | Proof |
|---|---|---|
| Structured tracing logs (env-filter) | ✅ | `tracing_subscriber` + `RUST_LOG` |
| Dev / prod run modes (Node.js-style multi-run: `dev`, `serve`, `daemon start/stop/restart/status`, `logs -f`) | ✅ | v0.3.1 — E2E daemon phase 7/7 |
| In-app updater (check → download → sha256 verify → swap → restart → auto-resume; PAT/private-repo aware) | ✅ | v0.3.1 — E2E mock-GitHub phase + CLI apply |
| Self-hosted update channel (fast `latest.json` mirror on istias.tech; no GitHub, no rate limits; CI-deployed on every release) | ✅ | v0.3.3 — `check_via_channel` unit tests + `scripts/e2e_update_channel.sh`; `docs/update-channel.md` |
| `GET /api/server` runtime info endpoint | ✅ | v0.3.1 — E2E UI phase |
| Sleep mode `--exit-when-idle <min>` (exit when nothing to do) | ✅ | v0.3.1 — E2E exit-when-idle phase |

---

## Roadmap — next version (v0.3.0 request) — SHIPPED in v0.3.1

### R1. Run modes — "like a Node.js app: multiple run options and see logs"

- **Dev mode** (`hyprfetch serve --mode dev` or `hyprfetch dev`): verbose
  debug tracing to the console, pretty log format, optional auto-open of the
  web UI; intended for running from a git clone so `git pull` + `cargo run`
  is the workflow.
- **Prod / daemon mode** (`hyprfetch serve --daemon`): detach from the
  terminal, write rotating log files under
  `~/.local/share/hyprfetch/logs/`, PID file for lifecycle commands.
- **Companion subcommands**: `hyprfetch logs [-f]`, `hyprfetch status`,
  `hyprfetch stop`, `hyprfetch restart` — same UX as Node process managers
  (pm2-style start/stop/logs).

### R2. In-app updater — "pull updates and apply in the app"

- **Check**: query GitHub releases (PAT-authenticated, private repo) or the
  local git clone; surface "new version available" in the UI settings page
  and `hyprfetch status`.
- **Apply (source install)**: `git pull` → `cargo build --release --locked`
  → swap binary → restart daemon.
- **Apply (binary install)**: download the new release tarball → verify
  sha256 → replace binary (sudo path note for `/usr/bin`) → restart daemon.
- **Safe apply**: drain active downloads first (pause), restart, auto-resume
  — the engine already resumes across restarts.

*Both items **shipped in v0.3.1** (2026-09-29): run modes (dev / serve /
daemon / logs), the PAT-aware self-updater with drain-pause → swap →
auto-resume, plus `--exit-when-idle` sleep mode, `--workers` runtime sizing,
`GET /api/server`, and the Tailwind+DaisyUI restyle. See `CHANGELOG.md`
§0.3.1 and `worktasks.md` §15 for the full proof list.*
