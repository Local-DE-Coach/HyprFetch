# API Reference

All endpoints are JSON. The server listens on `127.0.0.1:7780` by default.

## Auth

- Loopback (`127.0.0.1` / `::1`) binds: no auth required — the daemon is a
  local desktop service.
- Non-loopback binds (e.g. `0.0.0.0`): every `/api/*` and `/ws` request must
  present the API token, either as `Authorization: Bearer <token>` or (for
  browser WebSocket clients, which cannot set custom headers on the
  handshake) as `?access_token=<token>`.
- The token is generated automatically on the first `hyprfetch serve` run and
  stored at `~/.config/hyprfetch/token` (mode 0600). Override it with the
  `--api-token` flag / `HYPRFETCH_API_TOKEN` env var / `api_token` config key.
  Log lines state whether a token was generated and whether it is enforced.
- `/healthz` and the embedded SPA (`/`) stay open without a token so liveness
  probes and the UI page work; the UI's API calls require the token when the
  daemon is bound non-loopback.
- Unauthorized responses are `401` with code `unauthorized` and a
  `WWW-Authenticate: Bearer` header.

## REST endpoints

### List tasks

`GET /api/tasks?state=active|completed|all`

Response:
```json
{
  "tasks": [
    {
      "id": "01HVA8K7S9J0Z3M1N2P4Q5R6ST",
      "url": "https://example.com/file.iso",
      "filename": "file.iso",
      "save_path": "/home/user/Downloads/file.iso",
      "total_bytes": 5368709120,
      "downloaded_bytes": 3650722204,
      "state": "downloading",
      "segments_requested": 8,
      "created_at": 1769000000,
      "updated_at": 1769000300,
      "completed_at": null,
      "error": null
    }
  ]
}
```

Live speed and per-segment progress are available on the detail endpoint
(`GET /api/tasks/:id`) and via the `task:progress` WebSocket event.

### Add task(s)

`POST /api/tasks`

Request:
```json
{
  "urls": ["https://example.com/file.iso"],
  "category": "auto",
  "save_dir": null,
  "filename": null,
  "segments": 8,
  "qos_override": null,
  "headers": {},
  "start_now": true
}
```

Save-location resolution (in precedence order):

1. `save_dir` — **direct save**: used verbatim (tilde-expanded), skips
   auto-categorization.
2. `category` set to a name (`video`, `pictures`, `music`, `compress`,
   `documents`, `apps`, `other`) — saves into that category folder
   (`category_dir_<name>` override or `<base>/<name>`). `none`/`base`
   saves straight into the base dir.
3. `category` omitted or `"auto"` — sorted by filename extension when the
   `categorize` setting is on (default).
4. Otherwise the base download dir (`~/Downloads` by default).

The resolved directory is created if missing.

**Filename & extension auto-detection (v0.4.6).** When `filename` is not
provided, it is taken from the last URL path segment (query string and
fragments stripped, percent-decoded, sanitized — `…/images?q=tbn:ANd9…`
yields `images`). When the server's `Content-Type` maps to a known media
type and the name carries no known extension, the engine corrects the
filename and re-sorts the destination folder BEFORE any byte is written
(`images` + `image/jpeg` → `pictures/images.jpg`). Explicit `save_dir` or
forced categories are always respected — only the extension changes.
Names with a known extension (`.jpg`, `.pdf`, …) are never touched.

Response: `201 Created` with the newly created task objects (one per URL).
Each task object carries a read-only `category` field derived from the
filename extension.

### Get task detail

`GET /api/tasks/:id`

Returns the full task object including per-segment progress:

```json
{
  "task": { /* ...as above... */ },
  "segments": [
    { "id": 0, "start": 0, "end": 671088639, "current": 412355328, "state": "downloading", "speed_bps": 540123 },
    { "id": 1, "start": 671088640, "end": 1342177279, "current": 412355328, "state": "downloading", "speed_bps": 558912 },
    /* ... */
  ]
}
```

### Control task state

- `POST /api/tasks/:id/pause`
- `POST /api/tasks/:id/resume`
- `POST /api/tasks/:id/cancel`
- `POST /api/tasks/:id/retry` — re-run an **errored** task: clears the error,
  moves it back to `queued`, and the queue pump starts it when a
  `max_concurrent_tasks` slot is free. Persisted segment offsets are kept —
  if the remote is unchanged the retry resumes from the last written byte;
  if it changed, the coordinator's validator recheck discards the offsets
  and restarts from byte 0. Invalid from non-errored states (`409`).

All return `200 OK` with the updated task object, or `409 Conflict` if the state transition is invalid (e.g. pausing an already-completed task).

### Desktop file actions

- `POST /api/tasks/:id/reveal` — open the file's folder in the system file
  manager (the WebUI **GO** button). The file itself is not launched. Works
  for any task whose save folder exists on disk.
- `POST /api/tasks/:id/open` — open the downloaded file with the system's
  default application (the WebUI **Open** button). Only **finished**
  downloads qualify (`409 Conflict` otherwise).

Both return `200 OK` with `{"opened": true, "what": "file"|"folder",
"path": "…", "opener": "…"}`, `404` for a missing task, and `400` when the
path no longer exists on disk. The opener program is `xdg-open`, override
with the `HYPRFETCH_FILE_OPENER` environment variable (used by tests); the
process is spawned detached, so the HTTP call never blocks on a GUI app.

**Folder opening never opens a terminal (v0.4.6).** For folders (`reveal`,
and `open-folder` below) the server checks the desktop's default
`inode/directory` handler; when it is missing or resolves to a terminal
emulator (a common trap on minimal window-manager setups like Hyprland),
the first installed GUI file manager wins instead (nautilus, dolphin,
nemo, thunar, caja, pcmanfm-qt, pcmanfm, krusader, spacefm, doublecmd).

### Open a save folder

`POST /api/open-folder`:
```json
{ "path": "/home/user/Downloads/pictures" }
```
→ `200 OK` with the same shape as the desktop file actions.

Opens one of HyprFetch's save folders in the system file manager (the
clickable folder cards on the Dashboard). Only the base download dir and
the configured category folders (plus their children) are allowed — any
other path returns `400`, so the UI can never launch an arbitrary
directory.

### Resource usage (this app only)

`GET /api/system/usage` — RAM + CPU of the HyprFetch process itself, read
from the kernel's own accounting (`/proc/self/status`, `/proc/self/stat`):
```json
{
  "rss_bytes": 12582912,
  "peak_rss_bytes": 14680064,
  "cpu_percent": 0.42,
  "threads": 7,
  "uptime_secs": 3600,
  "quiet": false
}
```
`cpu_percent` is normalized to all cores (100 = every core fully busy) and
measured between successive calls.

### Background (quiet) mode

- `POST /api/power/quiet` — enter background mode: the server keeps
  running (downloads continue) while its own periodic work wakes 10× less
  often. Returns `{"quiet": true, "pid": …, "rss_bytes": …, "reopen":
  "hyprfetch open"}`.
- `POST /api/power/wake` — leave background mode.

The CLI equivalents are `hyprfetch close` (quiet mode for the running
daemon) and `hyprfetch open` (start if needed + open the web UI).

### Inspect a URL (confirm dialog)

`POST /api/inspect`:
```json
{ "url": "https://example.com/movie.mkv", "category": "auto" }
```
→
```json
{
  "url": "https://example.com/movie.mkv",
  "final_url": "https://cdn.example.com/movie.mkv",
  "filename": "movie.mkv",
  "total_bytes": 1468006400,
  "accept_ranges": true,
  "category": "video",
  "save_dir": "/home/user/Downloads/video",
  "save_path": "/home/user/Downloads/video/movie.mkv",
  "content_type": "video/mp4"
}
```

Probes the URL with a HEAD request using the same SSRF policy, redirect
handling and user agent as real downloads, and resolves where the file
would land (same precedence as `POST /api/tasks`: `save_dir` → `category` —
including `"auto"`, i.e. extension sorting — → auto-detect). **Creates
nothing**: no task row, no directories. Probe failures return `400`
(`403` when SSRF protection blocks the host); the WebUI falls back to a
local guess and lets the download proceed.

### Remove task

`DELETE /api/tasks/:id?delete_file=false`

Query param `delete_file=true` also removes the (partial) downloaded file
from disk. Best-effort: a missing file is not an error. Returns `204 No
Content` either way.

### QoS control

`GET /api/qos` → `{ "enabled": false, "target_bps": null }`

`PUT /api/qos`:
```json
{ "enabled": true, "target_bps": 5242880 }
```

If `target_bps` is omitted when enabling, defaults to 70% of measured max bandwidth (or 5 MB/s if no measurement exists yet).

### Settings

`GET /api/settings` → returns the settings map. Secret keys
(`github_token`, `api_token`) are **masked**: the raw value is replaced by
a `<key>_set` boolean (`"true"`/`"false"`).

`PATCH /api/settings` → partial update; secrets are accepted in the patch
and stored, but never echoed back. Patching any directory key
(`download_dir`, `categorize`, `category_dir_*`) rebuilds the folder
layout on disk immediately.

Enforcement status of the seeded keys (be honest in the UI):

| Key | Enforced |
|---|---|
| `qos_enabled`, `qos_target_bps` | yes — engine-wide limiter, restored at startup |
| `download_dir`, `segments_default` | yes — used by `POST /api/tasks` defaults |
| `categorize`, `category_dir_*` | yes — auto-sort + per-category folder overrides; folders auto-created |
| `github_token` | yes — updater token (masked on GET; env/CLI/config take precedence) |
| `max_concurrent_tasks` | yes — queue pump caps concurrently running tasks (0 = unlimited) |
| `user_agent` | yes — applied to outgoing requests at startup |
| `ssrf_block_private` | yes — combined with `--allow-private` at startup |
| `ui_theme_style` | yes — WebUI color style, synced to every browser (validated: `slate`/`ocean`/`forest`/`coffee`/`cyber`, empty = clear) |
| `ui_theme_mode` | yes — WebUI dark/light, synced to every browser (validated: `dark`/`light`) |
| `show_resource_usage` | yes — footer RAM/CPU widget (v0.4.6) |
| `keep_alive_in_background` | yes — controls the ⏾ close-to-background UI (v0.4.6) |
| `bind` | informational — the actual bind comes from `--bind` / config / default |
| `max_connections` | **not enforced yet** (planned global connection cap) |
| `protocol_pref` | **not enforced yet** (planned HTTP/2/3 selection) |

### Categories

`GET /api/categories` — the folder layout used to auto-sort downloads:

```json
{
  "base": "/home/user/Downloads",
  "categorize": true,
  "categories": [
    { "name": "video", "dir": "/home/user/Downloads/video", "overridden": false },
    { "name": "pictures", "dir": "/home/user/Downloads/pictures", "overridden": false },
    { "name": "music", "dir": "/home/user/Downloads/music", "overridden": false },
    { "name": "compress", "dir": "/home/user/Downloads/compress", "overridden": false },
    { "name": "documents", "dir": "/home/user/Downloads/documents", "overridden": false },
    { "name": "apps", "dir": "/home/user/Downloads/apps", "overridden": false },
    { "name": "other", "dir": "/home/user/Downloads/other", "overridden": false }
  ]
}
```

Folders are created automatically at server startup, on settings changes,
and when a task is created.

### Server info

`GET /api/server` — runtime info for `hyprfetch daemon status` and the UI
footer:

```json
{
  "version": "0.3.1",
  "uptime_secs": 42,
  "active_tasks": 1,
  "ws_clients": 2,
  "update_available": true,
  "latest_version": "0.3.2",
  "quiet": false
}
```

`update_available` / `latest_version` are `null` until a check has run.

### In-app updates

The updater has ONE source — the self-hosted update channel
(`https://istias.tech/hyprfetch/updates/latest.json`) that CI populates on
every release. One plain HTTPS GET, no GitHub API, no rate limits, works
even when the source repo is private. Install downloads the
manifest-listed archive and verifies the manifest sha256 before anything
touches disk. GitHub is never contacted.

`GET /api/update/check` → runs the check and caches it:

```json
{
  "current": "0.4.0",
  "latest": "0.5.0",
  "available": true,
  "published_at": "2026-09-29T00:00:00Z",
  "release_url": "https://istias.tech/hyprfetch/updates",
  "asset": { "name": "hyprfetch-0.5.0-linux-x64.tar.gz", "size": 3605057 },
  "channel": "https://istias.tech/hyprfetch/updates/"
}
```

When the channel cannot be reached the response carries
`"available": false`, `"latest": null`, a human-readable `"error"` and the
`"updates_page"` URL (<https://istias.tech/hyprfetch/updates>) for manual
steps. Override/disable the channel with `[update] channel` or
`HYPRFETCH_UPDATE_CHANNEL` (see `docs/update-channel.md`).

Every check/apply response also carries `"stale_copies"`: other
`hyprfetch` executables found on `PATH` that are not the running binary —
each with `path`, `shadows` (whether `PATH` resolves that copy before the
running one, so the old build keeps launching), `version` (self-reported
via a 2 s-capped probe) and `owned_by` (pacman package, when applicable).

`POST /api/update/apply?restart=true|false` → download → sha256 verify →
atomic binary swap (`hyprfetch.old` kept as rollback). System-owned
install locations are escalated **without a TTY**: passwordless `sudo -n`
first, then `pkexec` (the desktop polkit agent shows the graphical
password prompt); when neither answers, `400` with the actionable
`sudo hyprfetch update` hint. With `restart=true` (default) it then drains
(pause) active downloads, re-execs a fresh server with the same arguments
— which auto-resumes the paused tasks — and shuts this process down
gracefully. Requires the server to have been started through
`hyprfetch serve`/`dev` (CLI restarts keep their own args). Returns `400`
when no check has been cached yet — a web request cannot rebuild the
binary; run `hyprfetch update` instead.

`POST /api/update/stale-copies/fix` → removes every non-package-owned
stale copy reported by `stale_copies` (the classic case: an old
`install.sh` build in `/usr/local/bin` shadowing the pacman-managed
`/usr/bin` one). Returns `{ removed, failed, owned, message }`;
package-owned files are only reported (remove them via the package
manager).

`POST /api/update/restart` → just the drain → re-exec → auto-resume part,
without an update.

## Desktop widget (v0.5.0)

The Quickshell bar widget (illogical-impulse) is manageable from the WebUI
(Settings → Desktop widget). Everything lives under `$HOME` — the endpoints
never touch privileged paths.

`GET /api/widget/status` →
`{ qs_root, qs_found, quickshell_found, hyprfetch_found, installed,
integrated, bar_file, version, up_to_date, reload_hint }` — `qs_root` is
`~/.config/quickshell/ii` (override via `HYPRFETCH_QS_ROOT`), `installed`
means `modules/downloadManager/DownloadWidget.qml` exists, `integrated`
means the bar QML references `DownloadWidget`.

`POST /api/widget/install` → downloads `widget.tar.gz` from the update
channel (same channel as the app updater — no GitHub), extracts the
`downloadManager/` tree (regular files only, traversal refused), wires the
bar QML with a marker-based edit (import + `DownloadWidget {}` after the
`layoutDirection: Qt.RightToLeft` anchor; `.bak-hyprfetch` backup kept;
unknown layouts are left untouched and reported in `note`) and records the
state in `~/.local/share/download-manager/widget.json`. Returns the status
body plus `note` when the bar could not be auto-edited.

`POST /api/widget/uninstall` → removes the module dir, strips the marker
block + import from the bar QML (restoring it byte-identically) and deletes
the state file. Returns the status body plus `bar_reverted`.

The POSIX installer (`curl …/widget-install.sh | sh`) performs the same two
edits — the battery proves installer and API produce byte-identical bar
files.

## WebSocket

`GET /ws` — upgrade to WebSocket; the server pushes engine events as JSON
text frames so the UI never has to poll.

Server → client only. Client messages are ignored (protocol pings are
answered automatically). If a slow client lags, missed events are skipped —
progress is self-correcting because every event carries absolute totals.

### Server → client events

```json
{ "event": "task:progress", "task_id": "01HVA...", "downloaded_bytes": 3650722204, "total_bytes": 5368709120, "speed_bps": 4423456, "ts": 1769000000000 }
{ "event": "task:state", "task_id": "01HVA...", "state": "downloading", "error": null, "ts": 1769000000000 }
{ "event": "global:speed", "speed_bps": 8846912, "active_tasks": 2, "ts": 1769000000000 }
```

- `task:state` is emitted for `downloading`, `paused`, `removed`, `error`
  (with `error` populated) and `complete` transitions.
- `task:progress` is throttled to one event per 500ms per task and includes
  a per-task instantaneous `speed_bps`.
- `global:speed` is pushed every 1s while tasks are active, plus one final
  zero-speed event when the daemon goes idle.

## Embedded web UI

`GET /` serves a Svelte SPA compiled into the binary (rust-embed): active
list with live progress, an add-task modal, and a QoS bandwidth toggle,
all driven by the `/ws` stream above. Hashed assets under `/assets/*` are
served with `Cache-Control: immutable`; unknown extension-less paths fall
back to the SPA; unknown `/api/*` paths still return JSON 404s.

## Error responses

All errors follow this shape:

```json
{
  "error": {
    "code": "invalid_url",
    "message": "URL scheme must be http or https",
    "details": { "url": "ftp://example.com/file" }
  }
}
```

Standard codes: `invalid_url`, `ssrf_blocked`, `task_not_found`, `invalid_state_transition`, `rate_limited`, `disk_full`, `io_error`, `internal_error`.
