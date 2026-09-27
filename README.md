# HyprFetch

A minimal-RAM, fast, resumable internet download manager with a browser-based control panel.

Built for Arch Linux + Hyprland, but works anywhere a single static binary can run.

## Why

Most download managers are either:
- Heavy Electron apps (Motrix, JDownloader) — 200 MB+ RAM just to sit there
- CLI-only (wget, curl, aria2c) — no nice UI
- TUI apps (aria2curses) — terminal-bound, can't run in background alongside your editor/terminal workflow

HyprFetch is a single Rust binary that serves a web UI on `127.0.0.1`. You open the page in your browser; the binary does the downloading. Idle RAM target: < 10 MB.

## Features (planned)

- **Resumable downloads** — survive restarts, crashes, and interruptions via SQLite-backed segment state
- **Multi-connection segmented downloads** — split a file into N ranges, fetch in parallel, write via `pwrite` to one fd
- **HTTP/1.1, HTTP/2, HTTP/3 (QUIC)** — protocol auto-negotiation
- **QoS / bandwidth-sharing mode** — toggle that caps downloads to leave headroom for browsing, gaming, video calls
- **Browser-based UI** — Svelte SPA embedded in the binary, served from `127.0.0.1:<port>`
- **Single static binary** — no runtime deps, no shared libraries, no Electron

## Architecture

```
┌─────────────────────────────────────────────┐
│ Browser (Svelte SPA)                        │
│  ↕ WebSocket (progress) + REST (commands)   │
└─────────────────────────────────────────────┘
                  │
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

See [`docs/architecture.md`](docs/architecture.md) for what the system is,
[`docs/design.md`](docs/design.md) for why it is built this way, and
[`docs/install.md`](docs/install.md) for packaged installation options.

## Project layout

```
.
├── crates/
│   ├── hyprfetch/         # main binary (CLI + server entry)
│   ├── hyprfetch-core/    # download engine: segments, resume, QoS
│   ├── hyprfetch-db/      # SQLite persistence + migrations
│   └── hyprfetch-api/     # axum HTTP server + WebSocket
│       └── ui/            # Svelte SPA (src + committed dist/, embedded at compile time)
├── packaging/             # deb/rpm/Arch packaging used by the release workflow
├── docs/                  # architecture, design rationale, API, install, dev guide
└── .github/               # CI: fmt + clippy + test + audit; release builds packages
```

## Install

Fast paths per distribution (see [`docs/install.md`](docs/install.md) for all
options):

```bash
# Ubuntu / Debian
sudo apt install ./hyprfetch_<version>-1_amd64.deb

# Fedora / RHEL
sudo dnf install ./hyprfetch-<version>-1.x86_64.rpm

# Arch Linux — download the release PKGBUILD, then:
makepkg -si
```

## Build & run

Requirements: Rust stable (1.85+), Node 20+ (only for UI dev).

```bash
# Build everything (UI is embedded at compile time)
cargo build --release

# Run with defaults (binds 127.0.0.1:7780)
./target/release/hyprfetch

# Open the UI
xdg-open http://127.0.0.1:7780
```

For development with hot-reload on the frontend:

```bash
# Terminal 1: backend
cargo run -- serve

# Terminal 2: frontend dev server (proxies API to backend)
cd crates/hyprfetch-api/ui && npm install && npm run dev
```

## Configuration

HyprFetch reads `~/.config/hyprfetch/config.toml` if present. CLI flags override config. Run `hyprfetch --help` for the full list.

Key defaults:
- Bind: `127.0.0.1:7780` (loopback only by default — never expose to LAN without auth)
- Default download dir: `~/Downloads`
- Default segments per task: 8
- Max concurrent tasks: 3
- Max concurrent connections total: 24
- QoS: off by default

## Security

- Binds to loopback only by default
- Fine-grained PAT-style API token for any non-loopback access
- SSRF protection: rejects `file://`, `ftp://`, private IP ranges (RFC 1918, loopback, link-local)
- No telemetry, no phone-home, no auto-update

## Sandbox test results (2026-09-28)

End-to-end tests of a release build (`commit fcdb3c2`) in a clean Linux
sandbox, driven through the REST API — the same code path the browser UI
uses. Raw time-series and the harness scripts live in the test sandbox
(`testdata/*.json`, `scripts/test_harness.py`).

### Test environment

| Item | Value |
|---|---|
| CPU / RAM | 2 vCPU Intel Xeon, 4.1 GiB RAM |
| OS | Linux 5.10 x86_64 (container) |
| Build | `cargo build --release` (opt-level 3, thin LTO, stripped) |
| Test media | `http://ipv4.download.thinkbroadband.com:8080/1GB.zip` (1,073,725,334 B) and `https://ipv4-download.thinkbroadband.com/5GB.zip` (5,368,626,730 B) |
| Media note | Both files are **raw random data named `.zip`**, not real zip archives — integrity was verified by byte count + sha256 range comparison, not `unzip -t` |
| Baseline | single-connection curl: ~2.2 MB/s to this host; 4 parallel curl ranges: ~8.9 MB/s |

### Results

| # | Test | Result | Throughput | Server RSS |
|---|---|---|---|---|
| 1 | 1 GB, 8 segments, QoS cap 4 MiB/s → **complete** | 1,073.7 MB in 262 s | avg 4.10 MB/s (cap 4.19 → within 2.2%) | 6.4 – 17.1 MB |
| 2 | 5 GB, 8 segments, full speed, first leg → **paused at 180 s** | 2,789 MB downloaded | avg 15.5 MB/s, peak 18.6 MB/s | 7.5 – 13.1 MB |
| 3 | server process **killed**, fresh process started → startup resume pass (`resumed=1`) → **complete** | remaining 2,579 MB in 216 s | avg 11.9 MB/s (tail slows as segments drain) | 7.6 – 11.6 MB |
| 4 | Idle daemon | healthy | — | **6.2 MB** RSS (meets the < 10 MB target) |
| 5 | Embedded UI | `GET /` + hashed `/assets/*` serve correctly (200) | — | — |

**Integrity verification (byte-exact vs independent downloads):**
- 1 GB: full-file `sha256` of HyprFetch's output equals a separate 4-way
  parallel curl download of the same URL:
  `e5c9b51bdfaf6337202810c3bc8fa789ca7e059a62fe747ab7fd86cb547b2c00`
- 5 GB: size exact; 5 sampled 1 MiB ranges (offsets 0, 1 MiB, 2.68 GB,
  1.23e8, last MiB) all byte-identical to remote ranges. Full-file
  sha256: `b5907208d256713676a61636b0f3c993a66de3fe89e94ce2bfc07e207f951b65`

### What the tests found (and fixed)

1. **30-second total request timeout (critical, fixed in this repo).**
   The first runs failed identically at ~32 s: 1 GB at ~16 MB/s died at
   ~455 MB, the same download QoS-capped to 4 MiB/s died at ~130 MB —
   30 s × throughput in both cases. Root cause: `reqwest`'s client-level
   `.timeout(30s)` covers the entire request *including streaming the
   body*, so every segment worker aborted mid-transfer
   (`only 0 of 8 segments completed`). The client now uses
   `connect_timeout` + idle `read_timeout` only.
2. **One dropped connection failed the whole task (fixed).** With the
   timeout fixed, a real-world event surfaced: 4 of 8 long-lived
   connections were dropped ~150 s into a transfer
   (`only 4 of 8 segments completed`). Segment workers now retry
   transient failures (up to 6 attempts, exponential backoff 1→15 s)
   from the last written byte offset; the task then completes
   unattended.
3. **Resume across process death works as designed.** Pausing persisted
   8 per-segment offsets + ETag to SQLite; a *new* server process
   HEAD-probed the remote (validators matched), restarted workers from
   the stored offsets, and finished the 5 GB file with byte-exact
   content.

### Observations

- Segment scaling is near-linear on this host: 1 connection ≈ 2.2 MB/s →
  8 segments ≈ 15.5–16.9 MB/s aggregate (7–7.6×) on the same box, with
  RSS never exceeding ~17 MB regardless of file size (pwrite + streaming
  chunks; nothing is buffered in RAM).
- QoS accuracy: requested 4 MiB/s, measured 4.10 MB/s average over a
  full 262 s download — within 2.2% of target, aggregate across all 8
  segments (the shared engine-wide bucket, not per-segment).
- The test host (thinkbroadband) drops long-lived connections after
  ~2.5 min under load; with segment retry this is fully absorbed.
- `https://ipv4.download.thinkbroadband.com/...` (dotted host) serves a
  TLS certificate that does not match the hostname and is rejected by
  TLS validation — the dashed host `ipv4-download.thinkbroadband.com`
  used here matches the certificate. A download manager must fail that
  URL loudly, and HyprFetch does (TLS error propagates to the task).

### Known gaps found during testing

- `POST /api/tasks/:id/retry` is documented in `docs/api.md` but not
  implemented (marked as planned there now); re-running an errored task
  currently means delete + re-create.
- `DELETE /api/tasks/:id?delete_file=true` returns an empty body rather
  than a JSON document.

## Status

Pre-alpha. See [`CHANGELOG.md`](CHANGELOG.md) and the open issues / PRs for current state.

## License

MIT — see [`LICENSE`](LICENSE).
