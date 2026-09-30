# HyprFetch Quickshell bar widget

A single unified download-manager widget for the **illogical-impulse (end4)**
bar on Arch Linux + Hyprland. It is a full state machine —
**idle → hover/recent → input → downloading → completed → idle** — that adds
almost nothing on top of the already-running HyprFetch daemon (~13 MB RAM).

## RAM discipline

| State | Loaded | Approx RAM |
|---|---|---|
| idle | bar icon + file watcher only | ~0.5 MB |
| recent / input / downloading / completed | one lazy-loaded popup each | ≤ ~2 MB total, freed on close |

- **No extra processes** — no Python, no bash daemons, nothing but the
  on-demand `hyprfetch` CLI call when you actually start a download.
- **No polling** — the widget watches `~/.local/share/download-manager/status.json`
  with `FileView { watchChanges: true }`; the daemon rewrites that file on
  every real change (throttled to at most 2×/s, only while downloading).
- **Lazy everything** — every popup lives in a `LazyLoader` and is destroyed
  when its state ends.

## Install

From an extracted release tree (this directory):

```bash
./install.sh
```

Or directly from the update channel (no download needed):

```bash
curl -fsSL https://istias.tech/hyprfetch/updates/widget-install.sh | sh
```

Then add one line to `~/.config/quickshell/ii/modules/bar/Bar.qml` inside the
bar layout (see the installer output) and reload:

```bash
qs -c illogical-impulse kill && qs -c illogical-impulse &
```

Uninstall: `./install.sh --uninstall`

## Files

```text
downloadManager/
├── DownloadWidget.qml        # main controller: bar icon + state machine
├── components/
│   ├── RecentPopup.qml       # view 1: recent downloads + "+"
│   ├── InputPopup.qml        # view 2: URL field + Start
│   ├── ActivePopup.qml       # view 3: progress bars, speed, ETA
│   └── CompletionToast.qml   # view 4: auto-hide toast (2.5 s)
└── utils/
    └── DownloadProcess.qml   # runs `hyprfetch add/reveal` on demand
```

## Backend interface

The widget reads `$XDG_DATA_HOME/download-manager/status.json`
(default `~/.local/share/download-manager/status.json`), written by the
HyprFetch daemon (v0.4.8+):

```json
{
  "active_downloads": [
    { "id": "t_ab12", "filename": "arch.iso", "progress": 45.5,
      "speed": "2.5 MB/s", "eta": "00:02:15", "state": "downloading" }
  ],
  "recent_downloads": [
    { "id": "t_cd34", "filename": "video.mp4", "status": "completed",
      "timestamp": 1696000000 }
  ],
  "last_completed": { "filename": "video.mp4", "timestamp": 1696000000 }
}
```

It writes `hyprfetch add <url>` / `hyprfetch reveal <id>` — nothing else.
`jq`/`aria2` are **not** required.
