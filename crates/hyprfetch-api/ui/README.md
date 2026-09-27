# HyprFetch Web UI

Minimal Svelte SPA served by the HyprFetch binary (embedded at compile time
with `rust-embed`).

## Features

- **Active list** — live progress bars, per-task speed, pause/resume/cancel
- **Add modal** — paste one URL per line, optional save dir + segment count
- **QoS toggle** — one shared bandwidth cap for the whole daemon (MiB/s)
- Live updates over `/ws` (`task:progress`, `task:state`, `global:speed`) —
  no polling

## Layout

- `src/App.svelte` — the entire UI (single-file component)
- `src/api.js` — tiny REST + WebSocket client
- `dist/` — **committed** build output; this is what `rust-embed` embeds, so
  `cargo build` works without a Node toolchain

## Rebuilding

```sh
npm install
npm run build     # writes dist/
cargo build       # picks up the new dist via rust-embed
```

During UI development, `npm run dev` proxies `/api` and `/ws` to a locally
running `hyprfetch serve` (port 7780) for hot reload.
