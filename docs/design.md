# Design Notes & Rationale

This document records *why* HyprFetch is built the way it is — the language
choice, the architecture decisions, the QoS strategy, the UI structure, and
the honest trade-offs accepted along the way. For a description of *what the
system is*, see [`architecture.md`](architecture.md); for the HTTP surface,
see [`api.md`](api.md); for day-to-day hacking, see
[`development.md`](development.md).

Status markers throughout: **[done]** = shipped, **[planned]** = designed and
tracked, not yet implemented.

## Why Rust (and not Go or Python)

The headline goal is a minimal RAM footprint — a daemon that sits in the
background next to an editor, a terminal, and a browser without ever being
noticeable. That goal decided the language before anything else was
discussed.

- **RAM.** Rust has no garbage collector, no runtime scheduler payload, and
  no green-thread machinery of the kind Go carries. A minimal Rust HTTP
  server plus downloader idles at 2–5 MB RSS; Go typically sits at 10–30 MB
  before doing any work; Python with asyncio lands at 30–80 MB. The measured
  idle RSS of the shipped binary is 6.2 MB (see the README test section),
  comfortably inside the < 10 MB target.
- **Security.** Memory safety eliminates entire CWE classes — buffer
  overflows, use-after-free, data races — which matters for a network-facing
  daemon. Combined with a strict SSRF filter, a loopback-only bind, and no
  telemetry, the attack surface stays small.
- **Closeness to the metal.** Download tuning is explicit: buffer sizes,
  `SO_RCVBUF`, congestion-control friendliness, positional writes, and later
  HTTP/3 via `h3`/`quinn` if it proves worthwhile. Go can do much of this,
  but Rust lets every allocation be reasoned about.

The accepted trade-off is a steeper learning curve and the absence of a
mature "download-manager framework" in Rust the way aria2 exists in C++.
HyprFetch assembles good pieces instead — and the pieces are good. Python
was rejected outright for the RAM goal; it remains plausible only as a
sidecar for plugin ecosystems such as yt-dlp integration.

## Architecture in one diagram

```
┌─────────────────────────────────────────────┐
│ Browser (Svelte SPA)                        │
│  ↕ WebSocket (progress) + REST (commands)   │
└─────────────────────────────────────────────┘
                  │ HTTP (localhost only)
                  ▼
┌─────────────────────────────────────────────┐
│ Single Rust binary (hyprfetch)              │
│  ┌──────────┐  ┌──────────────────────────┐ │
│  │ axum     │  │ download engine          │ │
│  │ HTTP/WS  │  │  - task queue             │ │
│  │ + static │  │  - segmented downloader  │ │
│  │ files    │  │  - rate limiter (QoS)    │ │
│  └──────────┘  └──────────────────────────┘ │
│         │              │                     │
│         ▼              ▼                     │
│   embedded UI   SQLite (state, resume)      │
└─────────────────────────────────────────────┘
```

- **Single binary.** The web UI is compiled into the executable with
  `rust-embed`; there is no separate file server, no Node process, no
  runtime deps. **[done]**
- **Persistence.** SQLite via `rusqlite` (bundled, WAL mode) stores every
  task's URL, size, ETag/Last-Modified, and per-segment byte offsets.
  Persistence is debounced (500 ms) so the disk is not hammered per chunk.
  **[done]**
- **Concurrency.** One tokio runtime; each task owns N segment workers;
  progress flows through mpsc channels into the engine, then out to SQLite
  and the WebSocket bus. **[done]**

## Resume — the core feature

Resume works on two layers, and both are required for it to be honest.

1. **HTTP layer.** Use `Range: bytes=N-`. The server must advertise
   `Accept-Ranges: bytes`; this is checked on the initial probe. If the
   server does not support ranges, the task is downloaded single-segment and
   resume is simply not promised for it — the UI does not pretend otherwise.
   **[done]**
2. **Application layer.** Segment state (start, end, current offset) is
   persisted to SQLite. On startup, every incomplete task is reloaded,
   HEAD-probed, and re-issued from the last persisted offset via positional
   writes. **[done]**

**The gotcha that shaped the design:** the remote resource must not have
changed between sessions, or the resumed file would be silently corrupt.
HyprFetch validates `ETag` / `Last-Modified` / `Content-Length` at startup;
if the validators differ, stored offsets are discarded and the download
restarts from byte 0. **[done]**

## Segmented downloads

- Split the total size into N segments (default 8) and give each segment one
  `Range: bytes=A-B` connection. Segments write through `pwrite`
  (`FileExt::write_at`) to a shared `Arc<File>`, so no locks are needed.
- The file is pre-allocated with `ftruncate` so concurrent random writes do
  not fragment the filesystem. **[done]**
- Auto-tuning (split slow segments, reduce counts when the link saturates)
  is **[planned]**; the measured near-linear scaling (1 conn ≈ 2.2 MB/s → 8
  segs ≈ 15.5 MB/s on the test host) means fixed-8 is already fast.
- **HTTP/2 caveat:** HTTP/2 multiplexes streams over one TCP connection, so
  8 "connections" may actually be 8 streams inside one window. True
  parallelism may require multiple underlying TCP connections; this is the
  aria2-studied behavior, and protocol handling is **[planned]**.
- **HTTP/3 / QUIC** via `quinn`/`h3` is **[planned]** — better on flaky
  networks, eliminates head-of-line blocking, not required for v1.

## QoS — the differentiator

Most download managers do not bother with bandwidth sharing; HyprFetch makes
it the headline toggle. Two layers were considered:

- **Application-level token bucket (shipped, v1).** One engine-wide
  governor-backed bucket shared by *all* active segments — the aggregate
  daemon bandwidth is capped, not each task separately. `PUT /api/qos`
  retunes the bucket live; per-task `force_off` bypasses it. Measured
  accuracy over a full download: requested 4 MiB/s, got 4.10 MB/s average —
  within 2.2%. Reads are paced in small chunks rather than "read 1 MB, then
  sleep", which leaves headroom for interactive traffic. **[done]**
- **Kernel-level `tc` (future, optional).** `fq_codel`/`htb` via a helper
  with `CAP_NET_ADMIN` would push fairness to the packet level (DNS, ACKs,
  other apps included). Deliberately skipped until the app-level limiter
  proves insufficient. **[planned]**

A common rule of thumb encoded in the UI: when QoS is ON, cap downloads at a
fraction (e.g. 70%) of the measured maximum, leaving the rest for browsing,
gaming, and video calls. Enabling BBR (`net.ipv4.tcp_congestion_control=bbr`)
is recommended in the docs as a system-level complement.

## Crate choices

| Concern | Choice | Why |
|---|---|---|
| HTTP client | `reqwest` (rustls, streaming, ranges) | high-level, range-capable, streaming bodies |
| HTTP server | `axum` | WS + REST in one router, typed extracts |
| Async runtime | `tokio` (full) | the standard; multithread scheduler |
| State | `rusqlite` (bundled) | zero system deps, WAL, sync is fine at this rate |
| Rate limit | `governor` | real token bucket, no hand-rolled drift |
| UI embedding | `rust-embed` | hashed assets into the binary at compile time |
| Config/CLI | `clap` + `toml` + `serde` | derive-friendly, env-var overrides |
| Logging | `tracing` + env-filter | structured, per-crate levels |
| Positional I/O | `std::os::unix::fs::FileExt` | `pwrite` semantics without the `nix` dep |

## Prior art studied (not reinvented)

- **aria2** (C++) — the gold standard for segment splitting, multi-connection
  strategy, and resume logic. Its decisions inform the planner and the
  ETag-validation behavior.
- **gopeed** (Go + web UI) — closest architectural sibling; useful reference
  for API shape and UX flow.
- **Motrix** (Electron over aria2) — the cautionary tale: good UI patterns,
  200 MB+ RAM architecture to avoid.

## UI structure (browser SPA)

Five pages, three of which are v1. The shell is a thin topbar (QoS pill,
aggregate speed, Add button) and a collapsible sidebar.

| Page | Purpose | Status |
|---|---|---|
| Active | downloading / paused / queued cards with live progress | **[done]** |
| History | completed / failed / removed | **[planned]** (completed stay in Active for now) |
| Settings | paths, segments, QoS | **[done]** (minimal set) |
| Stats | bandwidth graphs, totals | **[planned]** |
| Logs | tail of tracing output | **[planned]** |

The download card is the workhorse component. It must render every state:
`queued` (gray), `downloading` (animated bar + speed), `paused` (frozen bar),
`complete` (green, "open folder" only), `error` (red bar + retry). An
expandable section shows per-segment mini-bars — power users love it, casual
users never open it. The Add modal accepts one URL per line (batch from day
one), with optional filename, save path, segment count, and QoS override;
submitting on Enter is the happy path.

## WebSocket event contract

The frontend ↔ backend contract, fixed early because it defines the whole
UI. Full DTOs live in [`api.md`](api.md); the shape:

```ts
// Server → client
{ "type": "task:progress", "id": "...", "downloaded": 0, "total": 0, "speed_bps": 0 }
{ "type": "task:state",    "id": "...", "state": "downloading" | "paused" | "complete" | "error", "error": "..." }
{ "type": "global:speed",  "speed": 0, "active_tasks": 0 }

// Client → server: REST calls (POST /api/tasks, POST /api/tasks/:id/pause|resume|cancel, PUT /api/qos)
```

Backpressure rule: `task:progress` is throttled server-side to one event per
500 ms per task; the bus drops events for slow clients rather than blocking
downloads, which is safe because every event carries absolute totals. **[done]**

## Honest gotchas (and what HyprFetch does about them)

- **Servers that lie about `Accept-Ranges`** — verified by behavior, not
  just headers; a server that ignores `Range` fails the task loudly instead
  of corrupting a resume.
- **CDNs shifting chunk boundaries between sessions** — ETag/Last-Modified
  validation resets offsets when the remote changes.
- **File descriptor limits** — 8 connections × many tasks adds up; the
  engine caps concurrent tasks and connections (settings), and packaging
  docs mention `ulimit` for heavy use.
- **Disk I/O as the real bottleneck** — segments buffer and write in larger
  batches; pre-allocation avoids seek storms on HDDs/SD cards.
- **WebSocket backpressure** — progress is throttled and dropped, never
  queued, so a hidden tab cannot slow a download.
- **SSRF** — every URL is validated (scheme, DNS-resolved IP, redirect hops
  re-checked); private/loopback/link-local ranges are blocked unless the
  operator explicitly opts in.

## v1 scope and what comes next

Shipped slice: segmented engine + byte-exact resume + shared QoS bucket +
WebSocket live events + embedded Svelte UI + REST API. Next up, in rough
priority order:

1. `POST /api/tasks/:id/retry` (documented, not yet implemented)
2. History page + retention policy
3. Per-task QoS UI (API already supports `force_off`)
4. Segment auto-tuning and HTTP/2/3 protocol handling
5. Stats page (60-second aggregate speed line, today/all-time totals)

Each item lands with tests, a CHANGELOG entry, and a worklog note — see
[`CONTRIBUTING.md`](../CONTRIBUTING.md) for the workflow.
