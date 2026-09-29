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
  "latest_version": "0.3.2"
}
```

`update_available` / `latest_version` are `null` until a check has run.

### In-app updates

The updater queries the latest GitHub release of the configured repo
(`[update] repo` in `config.toml`, default `Local-DE-Coach/HyprFetch`). For
private repos a PAT is resolved from `HYPRFETCH_GITHUB_TOKEN` / `GITHUB_TOKEN`
/ `[update] token` / the `github_token` setting. Release assets are
downloaded through the REST API octet-stream endpoint and sha256-verified
before anything touches disk.

`GET /api/update/check` → runs the check and caches it:

```json
{
  "current": "0.3.1",
  "latest": "0.3.2",
  "available": true,
  "published_at": "2026-09-29T00:00:00Z",
  "release_url": "https://github.com/…/releases/tag/v0.3.2",
  "asset": { "name": "hyprfetch-0.3.2-x86_64-unknown-linux-gnu.tar.gz", "size": 3605057, "id": 1001 }
}
```

`POST /api/update/apply?restart=true|false` → download → sha256 verify →
atomic binary swap (`hyprfetch.old` kept as rollback). With `restart=true`
(default) it then drains (pause) active downloads, re-execs a fresh server
with the same arguments — which auto-resumes the paused tasks — and shuts
this process down gracefully. Requires the server to have been started
through `hyprfetch serve`/`dev` (CLI restarts keep their own args).

`POST /api/update/restart` → just the drain → re-exec → auto-resume part,
without an update.

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
