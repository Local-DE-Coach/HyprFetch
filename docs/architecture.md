# Architecture

## Goals (in priority order)

1. **Minimal RAM footprint** — idle < 10 MB. Headline feature.
2. **Fast downloads** — multi-connection segmented, HTTP/2 + HTTP/3 aware, BBR-friendly.
3. **Resume any time** — every byte of progress is durable. Crash-safe, restart-safe.
4. **Secure** — loopback bind by default, SSRF-protected, no remote code paths.
5. **Browser-based UI** — single web page served by the binary. No native UI.

## Non-goals

- Bulk scraping / mirroring tools
- Torrent / DHT support (use transmission/qbittorrent)
- Browser extension integration (out of scope for v1)
- Cloud sync of download history

## Component breakdown

### `hyprfetch` (binary crate)

The entry point. Parses CLI args (clap), loads config, boots the tokio runtime, wires up `hyprfetch-core` + `hyprfetch-db` + `hyprfetch-api`, and serves until SIGINT.

### `hyprfetch-core` (library)

The download engine. Owns:
- `Task` — the in-memory representation of a download
- `Segment` — a byte range being fetched by one HTTP connection
- `Engine` — the orchestrator that owns the task queue, spawns segment workers, throttles total bandwidth via the QoS rate limiter, and persists progress to `hyprfetch-db`
- `QosLimiter` — token-bucket rate limiter shared across all active segments

The engine never touches HTTP directly — that's `hyprfetch-core`'s `HttpClient` abstraction (currently `reqwest`).

### `hyprfetch-db` (library)

SQLite persistence layer. Owns:
- Connection pool (rusqlite + WAL mode)
- Migration runner (applies SQL files from `migrations/`)
- Task / Segment / Settings repositories (CRUD)

Schema is intentionally simple — see `migrations/001_init.sql` for the full layout.

### `hyprfetch-api` (library)

HTTP + WebSocket server (axum). Owns:
- REST endpoints (`GET /api/tasks`, `POST /api/tasks`, `POST /api/tasks/:id/pause`, etc.)
- WebSocket endpoint (`/ws`) that fans out progress events
- Static file serving for the embedded SPA (via `rust-embed`)
- Auth middleware (token from `Authorization: Bearer` header — required only if bind is non-loopback)

## Concurrency model

- One `tokio` runtime, multi-threaded scheduler
- `Engine` holds an internal `HashMap<TaskId, TaskHandle>` behind a `RwLock`
- Each `Task` owns N segment worker tasks (one per active HTTP connection)
- Progress flows: segment worker → progress channel → engine → SQLite (debounced 500ms) + WebSocket (debounced 500ms per task)
- Backpressure: if the WebSocket receiver is slow (browser tab hidden), the engine drops progress events rather than blocking the download

## Memory budget (target)

| Component | RSS target |
|---|---|
| Tokio runtime (4 threads) | ~1 MB |
| axum + hyper server | ~1 MB |
| SQLite + WAL cache | ~1 MB |
| reqwest + TLS (rustls) | ~2 MB |
| Engine + task state (100 tasks × 8 segments) | ~1 MB |
| Embedded UI assets (gzip-compressed) | ~200 KB |
| Misc buffers (per-connection 64 KB × 24) | ~1.5 MB |
| **Total idle** | **~7–8 MB** |

Active downloads add ~500 KB per concurrent segment (mostly TLS state + read buffer).

## Resume design

On task creation:
1. `HEAD` request (or first `GET` with `Range: bytes=0-`) → learn `Content-Length`, `Accept-Ranges`, `ETag`, `Last-Modified`
2. If `Accept-Ranges: bytes` → split into N segments, persist each segment's `(start, end, current_offset=0, state=Pending)` to SQLite
3. `ftruncate` the target file to `Content-Length` so random writes don't fragment
4. Open the file once per task, share `Arc<File>` across segment workers

On segment progress (every 64 KB written):
1. `pwrite(buf, offset)` to the shared file
2. Update `current_offset` in memory
3. Mark `dirty=true` on the segment

Persistence tick (every 500 ms):
1. Iterate dirty segments → batched `UPDATE` to SQLite
2. Clear `dirty`

On restart:
1. Load all tasks with `state IN ('Paused', 'Downloading', 'Queued')`
2. For each task: `HEAD` the URL again, compare `ETag`/`Last-Modified` with stored value
3. If unchanged → resume from `current_offset` for each segment
4. If changed → log warning, mark task as `Error("resource changed")`, do not auto-restart
5. If server doesn't return `ETag`/`Last-Modified` → fall back to `Content-Length` match

## QoS design

Two-layered:

### Layer 1: Application token bucket (v1)

- `QosLimiter` holds a shared `governor::clock`-aware token bucket
- Capacity = 2 × target rate (allows short bursts)
- Refill rate = target rate (e.g. 70% of measured max)
- Every segment worker calls `limiter.acquire(bytes_to_read)` before each `read` from the socket
- When QoS is OFF, the limiter is bypassed (no allocation, no contention)

### Layer 2: Kernel `tc` (future, optional)

- Setuid helper binary or systemd unit with `CAP_NET_ADMIN`
- `tc qdisc add dev <iface> root fq_codel` for general fairness
- `tc class add ... htb rate <rate>` for hard cap on the download manager's traffic class
- Skipped for v1 — the app-level limiter is sufficient for the headline use case

## Threat model

- **Attacker**: anyone who can reach the HTTP port
- **Mitigation**: default bind to `127.0.0.1` only. If user explicitly opts into `0.0.0.0`, require an API token and log a warning at startup
- **Attacker**: malicious URL submitted via the API (SSRF)
- **Mitigation**: URL validation rejects `file://`, `ftp://`, `gopher://`, and any IP that resolves to private/loopback/link-local ranges (configurable). User can allowlist specific hosts
- **Attacker**: malicious redirect chain that escapes private IP filter
- **Mitigation**: redirect chain follows max 5 hops, each re-validated
- **Attacker**: path traversal via suggested filename
- **Mitigation**: filenames are sanitized — strip `..`, leading `/`, control chars; prefix with download dir
