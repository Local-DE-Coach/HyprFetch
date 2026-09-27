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

See [`docs/architecture.md`](docs/architecture.md) for details.

## Project layout

```
.
├── crates/
│   ├── hyprfetch/         # main binary (CLI + server entry)
│   ├── hyprfetch-core/    # download engine: segments, resume, QoS
│   ├── hyprfetch-db/      # SQLite persistence + migrations
│   └── hyprfetch-api/     # axum HTTP server + WebSocket
├── web/                   # Svelte SPA frontend
├── docs/                  # architecture, API, dev guide
└── .github/workflows/     # CI: fmt + clippy + test + audit
```

## Build & run

Requirements: Rust stable (1.75+), Node 20+ (only for UI dev).

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
cd web && npm install && npm run dev
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

## Status

Pre-alpha. See [`CHANGELOG.md`](CHANGELOG.md) and the open issues / PRs for current state.

## License

MIT — see [`LICENSE`](LICENSE).
