import QtQuick
import QtQuick.Layouts
import Quickshell
import Quickshell.Io
import qs.modules.common
import qs.modules.common.widgets
import "components"

// HyprFetch sidebar widget (illogical-impulse) — the "Downloads" tab of
// the left sidebar. Reads ~/.local/share/download-manager/status.json
// once per second (one `cat` per tick — negligible) and renders:
//
//   ┌ Download Manager ──────── ⟳ ┐
//   │ active + recent downloads   │
//   │ …                           │
//   ├ [ paste URL … ]  [Start] ───┤
//   └─────────────────────────────┘
//
// Backend JSON schema (hyprfetch-core/src/widget_status.rs):
//   active_downloads: [{id, filename, progress, speed, eta, state, path}]
//   recent_downloads: [{id, filename, status, timestamp, path}]
//   last_completed:   {filename, timestamp} | null
Item {
    id: root
    Layout.fillWidth: true
    Layout.fillHeight: true

    property var activeDownloads: []
    property var recentDownloads: []
    property var allDownloads: []

    Process {
        id: readStatus
        command: ["cat", Quickshell.env("HOME") + "/.local/share/download-manager/status.json"]
        running: false
        stdout: StdioCollector {
            onStreamFinished: {
                try {
                    let data = JSON.parse(text)
                    root.activeDownloads = data.active_downloads || []
                    root.recentDownloads = data.recent_downloads || []
                    root.allDownloads = root.activeDownloads.concat(root.recentDownloads)
                } catch (e) {
                    console.log("DownloadManager: error parsing status.json:", e)
                }
            }
        }
    }

    Timer {
        interval: 1000
        running: true
        repeat: true
        onTriggered: readStatus.running = true
    }

    Component.onCompleted: readStatus.running = true

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        DownloadHeader {
            Layout.fillWidth: true
            onRefreshRequested: {
                readStatus.running = false
                readStatus.running = true
            }
        }

        DownloadList {
            Layout.fillWidth: true
            Layout.fillHeight: true
            model: root.allDownloads
        }

        DownloadInputBar {
            Layout.fillWidth: true
        }
    }
}
