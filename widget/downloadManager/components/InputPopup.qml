import QtQuick
import QtQuick.Controls
import Quickshell
import qs.modules.common

/**
 * View 2 — URL input (triggered by the "+" in RecentPopup).
 *
 * Enter or "Start" runs `hyprfetch add <url>` (via DownloadProcess); on
 * success the widget's state flips to "downloading" — which also unloads
 * THIS popup from memory. On failure the popup stays open with the error.
 */
PopupWindow {
    id: popupRoot

    required property var controller

    visible: true
    width: 360
    height: bgCol.height + 20

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

        Column {
            id: bgCol
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: 12
            spacing: 10

            Text {
                text: "Add download"
                color: MaterialTheme.onSurface
                font.pixelSize: 13
                font.weight: Font.DemiBold
            }

            Rectangle {
                width: parent.width
                height: 36
                radius: 10
                color: Qt.alpha(MaterialTheme.surfaceVariant, 0.5)
                border.width: urlField.activeFocus ? 1 : 0
                border.color: MaterialTheme.primary

                TextField {
                    id: urlField
                    anchors.fill: parent
                    anchors.leftMargin: 10
                    anchors.rightMargin: 10
                    background: null
                    placeholderText: "https://example.com/file.zip"
                    placeholderTextColor: MaterialTheme.onSurfaceVariant
                    color: MaterialTheme.onSurface
                    font.pixelSize: 12
                    selectByMouse: true
                    echoMode: TextInput.Normal
                    onAccepted: popupRoot.start()
                    Keys.onEscapePressed: controller.state = "idle"

                    Component.onCompleted: forceActiveFocus()
                }
            }

            Text {
                width: parent.width
                visible: controller.lastError.length > 0
                text: controller.lastError
                color: MaterialTheme.error
                font.pixelSize: 11
                elide: Text.ElideRight
                wrapMode: Text.NoWrap
            }

            Item {
                width: parent.width
                height: 34

                // Start button (primary action).
                Rectangle {
                    anchors.right: parent.right
                    width: startArea.containsMouse ? parent.width * 0.42 : parent.width * 0.40
                    height: parent.height
                    radius: 12
                    color: MaterialTheme.primary

                    Behavior on width {
                        NumberAnimation { duration: 200; easing.type: Easing.OutCubic }
                    }

                    Text {
                        anchors.centerIn: parent
                        text: "Start"
                        color: MaterialTheme.onPrimary
                        font.pixelSize: 12
                        font.weight: Font.Medium
                    }

                    MouseArea {
                        id: startArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: popupRoot.start()
                    }
                }

                // Cancel (ghost).
                Rectangle {
                    anchors.left: parent.left
                    width: 84
                    height: parent.height
                    radius: 12
                    color: cancelArea.containsMouse
                        ? Qt.alpha(MaterialTheme.onSurface, 0.08)
                        : "transparent"

                    Text {
                        anchors.centerIn: parent
                        text: "Cancel"
                        color: MaterialTheme.onSurfaceVariant
                        font.pixelSize: 12
                    }

                    MouseArea {
                        id: cancelArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: controller.state = "idle"
                    }
                }
            }
        }
    }

    function start() {
        // On success the controller switches to "downloading", which
        // unloads this popup; on failure the error binding lights up.
        controller.startDownload(urlField.text);
    }
}
