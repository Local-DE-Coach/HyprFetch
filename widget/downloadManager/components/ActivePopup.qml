import QtQuick
import Quickshell
import qs.modules.common

/**
 * View 3 — Active downloads: progress bars, speed, ETA.
 *
 * Lazy-loaded while the widget's state is "downloading". Data comes from
 * the watched status file (controller.activeDownloads) — each backend
 * write updates the bars in place; the fill width animates 200 ms OutCubic.
 */
PopupWindow {
    id: popupRoot

    required property var controller

    readonly property var shown: controller.activeDownloads.slice(0, 4)
    readonly property int overflow: Math.max(0, controller.activeDownloads.length - 4)

    visible: true
    width: 340
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

            Repeater {
                model: popupRoot.shown

                delegate: Column {
                    id: rowCol
                    required property var modelData
                    width: parent.width
                    spacing: 5

                    // Name + state tag.
                    Item {
                        width: parent.width
                        height: 18

                        Text {
                            id: nameText
                            anchors.left: parent.left
                            anchors.right: pausedTag.visible ? pausedTag.left : parent.right
                            anchors.rightMargin: pausedTag.visible ? 8 : 0
                            anchors.verticalCenter: parent.verticalCenter
                            text: rowCol.modelData.filename
                            elide: Text.ElideMiddle
                            color: MaterialTheme.onSurface
                            font.pixelSize: 12
                        }

                        Rectangle {
                            id: pausedTag
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            visible: rowCol.modelData.state === "paused" || rowCol.modelData.state === "queued"
                            width: tagText.implicitWidth + 12
                            height: 16
                            radius: 8
                            color: Qt.alpha(MaterialTheme.tertiary, 0.25)

                            Text {
                                id: tagText
                                anchors.centerIn: parent
                                text: rowCol.modelData.state === "paused" ? "paused" : "queued"
                                color: MaterialTheme.tertiary
                                font.pixelSize: 10
                            }
                        }
                    }

                    // Progress bar (custom — track + animated fill).
                    Item {
                        width: parent.width
                        height: 6

                        Rectangle {
                            id: track
                            anchors.fill: parent
                            radius: 3
                            color: Qt.alpha(MaterialTheme.onSurface, 0.12)
                        }

                        Rectangle {
                            anchors.left: parent.left
                            anchors.top: parent.top
                            anchors.bottom: parent.bottom
                            radius: 3
                            width: track.width * Math.min(1, Math.max(0, (rowCol.modelData.progress ?? 0) / 100))
                            color: MaterialTheme.primary

                            Behavior on width {
                                NumberAnimation { duration: 200; easing.type: Easing.OutCubic }
                            }
                        }
                    }

                    // Speed + ETA.
                    Item {
                        width: parent.width
                        height: 14

                        Text {
                            anchors.left: parent.left
                            anchors.verticalCenter: parent.verticalCenter
                            text: rowCol.modelData.speed
                            color: MaterialTheme.onSurfaceVariant
                            font.pixelSize: 11
                        }

                        Text {
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            text: rowCol.modelData.progress > 0
                                ? Math.round(rowCol.modelData.progress) + "% · " + rowCol.modelData.eta
                                : rowCol.modelData.eta
                            color: MaterialTheme.onSurfaceVariant
                            font.pixelSize: 11
                        }
                    }
                }
            }

            Text {
                visible: popupRoot.shown.length === 0
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                text: "Waiting for the download to start…"
                color: MaterialTheme.onSurfaceVariant
                font.pixelSize: 12
                topPadding: 6
                bottomPadding: 6
            }

            Text {
                visible: popupRoot.overflow > 0
                width: parent.width
                horizontalAlignment: Text.AlignHCenter
                text: "+" + popupRoot.overflow + " more — open the web UI"
                color: MaterialTheme.onSurfaceVariant
                font.pixelSize: 11
            }
        }
    }
}
