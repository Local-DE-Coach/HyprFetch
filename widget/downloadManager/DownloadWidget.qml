import QtQuick
import Quickshell
import Quickshell.Io
import qs.modules.common
import "components"
import "utils"

/**
 * HyprFetch bar widget — unified download-manager state machine for the
 * illogical-impulse (end4) Quickshell bar.
 *
 * Flow: idle → hover/recent → input → downloading → completed → idle.
 *
 * RAM discipline (the whole point):
 * - At idle only the bar icon + one file watcher exist (~0.5 MB).
 * - Every popup lives inside a LazyLoader and is DESTROYED when its state
 *   ends — RAM drops back to the idle footprint.
 * - Updates arrive by WATCHING the backend's status file
 *   (~/.local/share/download-manager/status.json) — the daemon pushes file
 *   changes, the widget never polls. A slow 5 s fallback runs ONLY while
 *   the file is missing (daemon off) and stops as soon as it appears.
 * - No timers, no extra processes: starting a download shells out to
 *   `hyprfetch add <url>` (the daemon already runs at ~13 MB).
 */
Item {
    id: root

    // ---- state exposed to the popups (parsed from the status file) ----
    property var activeDownloads: []
    property var recentDownloads: []
    property var lastCompleted: null
    property string lastSeenCompletionTs: ""
    property bool popupHovered: false
    property string lastError: ""
    readonly property int activeCount: activeDownloads.length
    /// Exposed so popups can cancel/restart the hover-intent close.
    property alias hoverCloseTimer: hoverCloseTimerImpl

    implicitWidth: 36
    implicitHeight: 28

    Component.onCompleted: root.state = "idle"

    // Single state machine — see the `states` array below. The default
    // (unnamed) state is "" until we switch to the named idle state.
    states: [
        State { name: "idle" },
        State { name: "recent" },
        State { name: "input" },
        State { name: "downloading" },
        State { name: "completed" }
    ]

    // ---------------- status file (watched, not polled) ----------------
    readonly property string statusPath: {
        const dataHome = Quickshell.env("XDG_DATA_HOME");
        const base = (dataHome && dataHome.length > 0)
            ? dataHome
            : (Quickshell.env("HOME") + "/.local/share");
        return base + "/download-manager/status.json";
    }

    FileView {
        id: statusFile
        path: root.statusPath
        watchChanges: true
        printErrors: false
        onLoaded: root.parseStatus()
        onFileChanged: root.parseStatus()
        onLoadFailed: () => {
            // Daemon not running (yet) — clear the view and retry slowly.
            // This is the ONLY fallback timer in the widget and it stops
            // the moment the file loads.
            root.activeDownloads = [];
            root.recentDownloads = [];
            root.lastCompleted = null;
            root.syncFlow();
            fallbackTimer.start();
        }
    }

    Timer {
        id: fallbackTimer
        interval: 5000
        repeat: true
        onTriggered: statusFile.reload()
    }

    // Self-heal for torn reads of the in-place-written status file.
    Timer {
        id: reparseTimer
        interval: 700
        onTriggered: root.parseStatus()
    }

    // Completion toast lifetime (2.5 s per spec).
    Timer {
        id: toastTimer
        interval: 2500
        onTriggered: if (root.state === "completed") root.state = "idle"
    }

    // Hover-intent close for the recent popup (icon ↔ popup bridge).
    Timer {
        id: hoverCloseTimerImpl
        interval: 350
        onTriggered: {
            if (root.state === "recent" && !iconArea.containsMouse && !root.popupHovered)
                root.state = "idle";
        }
    }

    function parseStatus() {
        let txt;
        try {
            // FileView.text is an invokable in current quickshell; tolerate
            // builds where it is a plain property.
            txt = (typeof statusFile.text === "function") ? statusFile.text() : statusFile.text;
        } catch (e) {
            reparseTimer.restart();
            return;
        }
        if (!txt || txt.length === 0) {
            fallbackTimer.start();
            return;
        }
        fallbackTimer.stop();
        let data;
        try {
            data = JSON.parse(txt);
        } catch (e) {
            // The daemon writes in place (truncate+write); a torn read is
            // possible for a microsecond — retry shortly.
            reparseTimer.restart();
            return;
        }
        root.activeDownloads = Array.isArray(data.active_downloads) ? data.active_downloads : [];
        root.recentDownloads = Array.isArray(data.recent_downloads) ? data.recent_downloads : [];
        root.lastCompleted = data.last_completed ?? null;
        root.syncFlow();
    }

    function syncFlow() {
        if (root.state === "idle") {
            if (root.activeCount > 0)
                root.state = "downloading";
        } else if (root.state === "downloading") {
            if (root.activeCount === 0) {
                const lc = root.lastCompleted;
                if (lc && String(lc.timestamp) !== root.lastSeenCompletionTs) {
                    root.lastSeenCompletionTs = String(lc.timestamp);
                    root.state = "completed";
                    toastTimer.restart();
                } else {
                    root.state = "idle";
                }
            }
        }
    }

    onActiveDownloadsChanged: root.syncFlow()

    // ---------------- popup anchoring (below the icon) ----------------
    function popupAnchorX(popupWidth) {
        const win = root.QsWindow.window;
        const p = iconItem.mapToItem(null, 0, 0);
        const x = p.x + iconItem.width / 2 - popupWidth / 2;
        if (win)
            return Math.max(8, Math.min(x, win.width - popupWidth - 8));
        return Math.max(8, x);
    }

    function popupAnchorY() {
        const p = iconItem.mapToItem(null, 0, 0);
        return p.y + iconItem.height + 6;
    }

    // ---------------- starting downloads ----------------
    // Returns true when the download was handed to the backend.
    function startDownload(url) {
        root.lastError = "";
        const trimmed = (url || "").trim();
        if (!(trimmed.startsWith("http://") || trimmed.startsWith("https://"))) {
            root.lastError = "Enter a valid http(s) URL";
            return false;
        }
        addProcess.addToQueue(trimmed);
        return true;
    }

    DownloadProcess {
        id: addProcess
        onFinished: (ok, output) => {
            if (ok) {
                root.lastError = "";
                root.state = "downloading";
            } else {
                root.lastError = (output && output.length > 0)
                    ? output
                    : "Could not add the download — is hyprfetch installed?";
            }
        }
    }

    // Fire-and-forget reveal (open the folder of a finished download).
    DownloadProcess {
        id: revealProcess
        onFinished: (ok, output) => {
            if (!ok)
                console.warn("[hyprfetch] reveal failed:", output);
        }
    }

    function revealTask(taskId) {
        if (!taskId)
            return;
        revealProcess.runCommand(["hyprfetch", "reveal", String(taskId)]);
    }

    // ---------------- bar icon ----------------
    Rectangle {
        anchors.fill: parent
        radius: 8
        color: (iconArea.containsMouse || root.popupHovered)
            ? Qt.alpha(MaterialTheme.secondaryContainer, 0.6)
            : "transparent"

        Behavior on color {
            ColorAnimation { duration: 200; easing.type: Easing.OutCubic }
        }
    }

    Text {
        id: iconItem
        anchors.centerIn: parent
        text: "\uf019"
        font.family: "Symbols Nerd Font"
        font.pixelSize: 15
        color: root.activeCount > 0 ? MaterialTheme.primary : MaterialTheme.onSurfaceVariant
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
    }

    // Tiny activity dot — the only "live" element at idle.
    Rectangle {
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: 3
        width: 7
        height: 7
        radius: 3.5
        visible: root.activeCount > 0
        color: MaterialTheme.primary
    }

    MouseArea {
        id: iconArea
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton
        onEntered: {
            hoverCloseTimerImpl.stop();
            if (root.state === "idle")
                root.state = "recent";
        }
        onExited: {
            if (root.state === "recent")
                hoverCloseTimerImpl.restart();
        }
        onClicked: {
            hoverCloseTimerImpl.stop();
            if (root.state === "idle")
                root.state = "recent";
            else if (root.state === "recent" || root.state === "input")
                root.state = "idle";
            // "downloading"/"completed" manage themselves via the status file.
        }
    }

    // ---------------- state machine views (lazy) ----------------
    LazyLoader {
        id: recentLoader
        active: root.state === "recent"
        RecentPopup { controller: root }
    }

    LazyLoader {
        id: inputLoader
        active: root.state === "input"
        InputPopup { controller: root }
    }

    LazyLoader {
        id: activeLoader
        active: root.state === "downloading"
        ActivePopup { controller: root }
    }

    LazyLoader {
        id: toastLoader
        active: root.state === "completed"
        CompletionToast { controller: root }
    }
}
