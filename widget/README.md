# HyprFetch Quickshell sidebar widget

The HyprFetch download manager as a **"Downloads" tab in the illogical-impulse
(end4) left sidebar** — Arch Linux + Hyprland. One tab, four components:

- **Header** — title + manual refresh
- **List** — active downloads (progress bar, speed, ETA) on top, then recent
  completed/errored ones; completed rows get Open File / Open Location /
  Remove buttons
- **Input bar** — paste a URL, confirm the exact save path in a dialog
  (pre-filled with `~/Downloads/<name>`), Start

The tab reads `~/.local/share/download-manager/status.json` once per second
(a single `cat` per tick — negligible CPU/RAM) and starts downloads with
`hyprfetch add <url> -o <path>`. The daemon already runs at ~13 MB RAM; the
widget adds nothing meaningful on top.

## Install

From an extracted release tree (this directory):

```bash
./install.sh
```

Or directly from the update channel (no download needed) — works under
`sh`, `dash`, `bash`, `zsh` or any POSIX shell:

```bash
curl -fsSL https://istias.tech/hyprfetch/updates/install.sh | sh   # HyprFetch itself
curl -fsSL https://istias.tech/hyprfetch/updates/widget-install.sh | sh   # this widget
```

The installer:

1. **removes the old v0.5.0 bar widget** if present (module dir + the
   auto-added bar edit) — one clean upgrade path,
2. copies the QML files into
   `~/.config/quickshell/ii/modules/ii/sidebarLeft/downloadManager/`,
3. **wires the Downloads tab into `SidebarLeftContent.qml`** — five surgical,
   idempotent edits (import, policy flag, tab entry, page instance,
   component); the original is saved once as
   `SidebarLeftContent.qml.bak-hyprfetch` and unknown layouts are left
   untouched with printed instructions,
4. sets `policies.downloadManager: 1` in
   `~/.config/illogical-impulse/config.json`,
5. creates the initial status file if missing and restarts Quickshell
   (`--no-restart` to skip; the systemd `quickshell.service` is restarted
   automatically when active).

You can also install it with one click from the WebUI:
**Settings → Desktop widget**.

Uninstall: `./install.sh --uninstall` (or the WebUI Remove button) — it
removes the files AND reverts the sidebar edit byte-exactly.

## Files

```text
downloadManager/
├── DownloadManager.qml            # tab root: 1 s status.json poll + layout
└── components/
    ├── DownloadHeader.qml         # title + refresh button
    ├── DownloadList.qml           # ListView over active + recent
    ├── DownloadItem.qml           # one row: progress, actions, remove confirm
    └── DownloadInputBar.qml       # URL field + confirm-path dialog
```

## Backend interface

The widget reads `$XDG_DATA_HOME/download-manager/status.json`
(default `~/.local/share/download-manager/status.json`), written by the
HyprFetch daemon (v0.4.8+; `path` added in v0.5.1):

```json
{
  "active_downloads": [
    { "id": "t_ab12", "filename": "arch.iso", "progress": 45.5,
      "speed": "2.5 MB/s", "eta": "00:02:15", "state": "downloading",
      "path": "/home/u/Downloads/arch.iso" }
  ],
  "recent_downloads": [
    { "id": "t_cd34", "filename": "video.mp4", "status": "completed",
      "timestamp": 1696000000, "path": "/home/u/Downloads/video.mp4" }
  ],
  "last_completed": { "filename": "video.mp4", "timestamp": 1696000000 }
}
```

It writes `hyprfetch add <url> -o <path>`, `hyprfetch remove <id>` —
nothing else. `jq`/`aria2` are **not** required.
