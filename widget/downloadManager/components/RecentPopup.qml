import QtQuick
import Quickshell
import qs.modules.common

/**
 * View 1 — Recent downloads + "+" button (hover/click state).
 *
 * Lazy-loaded: exists only while the widget's state is "recent"; moving
 * the mouse away (or clicking elsewhere) unloads the whole popup window.
 */
PopupWindow {
    id: popupRoot

    required property var controller

    visible: true
    width: 320
    height: bgCol.height + 20

    anchor.window: controller.QsWindow.window
    anchor.rect.x: controller.popupAnchorX(width)
    anchor.rect.y: controller.popupAnchorY()

    color: "transparent"

    readonly property var recentItems: controller.recentDownloads.slice(0, 5)

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
            onHoveredChanged: {
                controller.popupHovered = hovered;
                if (!hovered)
                    controller.hoverCloseTimer.restart();
            }
        }

        Column {
            id: bgCol
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: 12
            spacing: 8

            Item {
                width: parent.width
                height: 20

                Text {
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                    text: "Downloads"
                    color: MaterialTheme.onSurface
                    font.pixelSize: 13
                    font.weight: Font.DemiBold
                }

                Text {
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    visible: controller.activeCount > 0
                    text: controller.activeCount + " active"
                    color: MaterialTheme.primary
                    font.pixelSize: 11
                }
            }

            Rectangle {
                width: parent.width
                height: 1
                color: Qt.alpha(MaterialTheme.outline, 0.2)
            }

            // Recent rows (max 5) — plain objects parsed from status.json.
            Column {
                width: parent.width
                spacing: 2
                visible: popupRoot.recentItems.length > 0

                Repeater {
                    model: popupRoot.recentItems

                    delegate: Item {
                        required property var modelData
                        width: parent.width
                        height: 36

                        Rectangle {
                            anchors.fill: parent
                            radius: 10
                            color: rowArea.containsMouse
                                ? Qt.alpha(MaterialTheme.onSurface, 0.06)
                                : "transparent"
                        }

                        Rectangle {
                            id: dot
                            anchors.left: parent.left
                            anchors.leftMargin: 8
                            anchors.verticalCenter: parent.verticalCenter
                            width: 8
                            height: 8
                            radius: 4
                            color: modelData.status === "completed"
                                ? MaterialTheme.primary
                                : MaterialTheme.error
                        }

                        Text {
                            anchors.left: dot.right
                            anchors.leftMargin: 10
                            anchors.right: statusLabel.left
                            anchors.rightMargin: 8
                            anchors.verticalCenter: parent.verticalCenter
                            text: modelData.filename
                            elide: Text.ElideMiddle
                            color: MaterialTheme.onSurface
                            font.pixelSize: 12
                        }

                        Text {
                            id: statusLabel
                            anchors.right: parent.right
                            anchors.rightMargin: 10
                            anchors.verticalCenter: parent.verticalCenter
                            text: modelData.status === "completed" ? "done" : "failed"
                            color: modelData.status === "completed"
                                ? MaterialTheme.onSurfaceVariant
                                : MaterialTheme.error
                            font.pixelSize: 11
                        }

                        MouseArea {
                            id: rowArea
                            anchors.fill: parent
                            hoverEnabled: true
                            // Clicking a finished download opens the folder
                            // it landed in — via the daemon's reveal API.
                            onClicked: {
                                if (modelData.id)
                                    controller.revealTask(modelData.id);
                            }
                        }
                    }
                }
            }

            Text {
                visible: popupRoot.recentItems.length === 0
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                text: "No recent downloads"
                color: MaterialTheme.onSurfaceVariant
                font.pixelSize: 12
                topPadding: 6
                bottomPadding: 6
            }

            // The "+" action — switches the widget into the input state.
            Rectangle {
                width: parent.width
                height: 38
                radius: 12
                color: addArea.containsMouse
                    ? MaterialTheme.secondaryContainer
                    : Qt.alpha(MaterialTheme.secondaryContainer, 0.55)

                Behavior on color {
                    ColorAnimation { duration: 200; easing.type: Easing.OutCubic }
                }

                Text {
                    anchors.centerIn: parent
                    text: "+  Add download"
                    color: MaterialTheme.onSecondaryContainer
                    font.pixelSize: 12
                    font.weight: Font.Medium
                }

                MouseArea {
                    id: addArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: {
                        controller.lastError = "";
                        controller.state = "input";
                    }
                }
            }
        }
    }
}
