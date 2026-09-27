# API Reference

All endpoints are JSON. The server listens on `127.0.0.1:7780` by default.

## Auth

- Loopback (`127.0.0.1` / `::1`): no auth required
- Non-loopback: requires `Authorization: Bearer <token>` header. Token is generated on first run and stored at `~/.config/hyprfetch/token`. If you bind to `0.0.0.0` without setting a token, the server refuses to start.

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
      "segments_total": 8,
      "segments_active": 8,
      "speed_bps": 4423456,
      "eta_sec": 387,
      "created_at": "2026-09-27T10:23:00Z",
      "completed_at": null,
      "error": null
    }
  ]
}
```

### Add task(s)

`POST /api/tasks`

Request:
```json
{
  "urls": ["https://example.com/file.iso"],
  "save_dir": "/home/user/Downloads",
  "filename": null,
  "segments": 8,
  "qos_override": null,
  "headers": {},
  "start_now": true
}
```

Response: `201 Created` with the newly created task objects (one per URL).

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
- `POST /api/tasks/:id/retry`

All return `200 OK` with the updated task object, or `409 Conflict` if the state transition is invalid (e.g. pausing an already-completed task).

### Remove task

`DELETE /api/tasks/:id?delete_file=false`

Query param `delete_file=true` also removes the partial file from disk.

### QoS control

`GET /api/qos` → `{ "enabled": false, "target_bps": null }`

`PUT /api/qos`:
```json
{ "enabled": true, "target_bps": 5242880 }
```

If `target_bps` is omitted when enabling, defaults to 70% of measured max bandwidth (or 5 MB/s if no measurement exists yet).

### Settings

`GET /api/settings` → returns the full settings object.

`PATCH /api/settings` → partial update. Some settings require a restart to take effect (noted in the response).

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
