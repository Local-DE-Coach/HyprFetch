import QtQuick
import Quickshell
import qs.modules.common

/**
 * View 4 — Completion toast: auto-hides after 2.5 s (controller toastTimer)
 * and the widget returns to idle. Lazy-loaded for the duration of the
 * "completed" state only.
 */
PopupWindow {
    id: popupRoot

    required property var controller

    readonly property string filename: {
        const lc = controller.lastCompleted;
        return (lc && lc.filename) ? String(lc.filename) : "download";
    }

    visible: true
    width: 300
    height: 64

    anchor.window: controller.QsWindow.window
    anchor.rect.x: controller.popupAnchorX(width)
    anchor.rect.y: controller.popupAnchorY()

    color: "transparent"

    Rectangle {
        id: bg
        anchors.fill: parent
        radius: 16
        color: Qt.alpha(MaterialTheme.surfaceContainer, 0.8)
        border.width: 1
        border.color: Qt.alpha(MaterialTheme.outline, 0.25)

        opacity: 0
        scale: 0.96
        Component.onCompleted: appear.restart()
        ParallelAnimation {
            id: appear
            NumberAnimation { target: bg; property: "opacity"; from: 0; to: 1; duration: 200; easing.type: Easing.OutCubic }
            NumberAnimation { target: bg; property: "scale"; from: 0.96; to: 1; duration: 200; easing.type: Easing.OutCubic }
        }

        HoverHandler {
            onHoveredChanged: controller.popupHovered = hovered
        }

        Row {
            anchors.fill: parent
            anchors.margins: 12
            spacing: 10

            // Check badge.
            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                width: 32
                height: 32
                radius: 16
                color: MaterialTheme.primary

                Text {
                    anchors.centerIn: parent
                    text: "✓"
                    color: MaterialTheme.onPrimary
                    font.pixelSize: 15
                    font.weight: Font.Bold
                }
            }

            Column {
                anchors.verticalCenter: parent.verticalCenter
                spacing: 2
                width: parent.width - 42

                Text {
                    width: parent.width
                    text: "Download complete"
                    color: MaterialTheme.onSurface
                    font.pixelSize: 12
                    font.weight: Font.Medium
                }

                Text {
                    width: parent.width
                    text: popupRoot.filename
                    elide: Text.ElideMiddle
                    color: MaterialTheme.onSurfaceVariant
                    font.pixelSize: 11
                }
            }
        }
    }
}
