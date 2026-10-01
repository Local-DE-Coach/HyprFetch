import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell
import qs.modules.common
import qs.modules.common.widgets

ListView {
    id: root
    clip: true
    spacing: 10
    topMargin: 10

    StyledText {
        anchors.centerIn: parent
        text: "No downloads found"
        color: Appearance.colors.colSubtext
        visible: root.count === 0
    }

    delegate: DownloadItem {
        width: root.width
        filename: modelData.filename || "Unknown file"
        downloadId: modelData.id || ""
        status: modelData.status || "downloading"
        progress: modelData.progress !== undefined ? (modelData.progress / 100) : 1.0
        speed: modelData.speed || ""
        eta: modelData.eta || ""
        filePath: modelData.path || (Quickshell.env("HOME") + "/Downloads/" + (modelData.filename || ""))
    }
}
