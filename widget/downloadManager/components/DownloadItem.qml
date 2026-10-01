import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell
import qs.modules.common
import qs.modules.common.widgets

Rectangle {
    id: root
    property string filename: ""
    property real progress: 0.0
    property string speed: ""
    property string eta: ""
    property string status: "downloading"
    property string filePath: ""
    property string downloadId: ""

    property bool isCompleted: status === "completed"
    property bool hovered: false

    height: isCompleted ? 75 : 90
    color: hovered ? Appearance.colors.colLayer2Hover : Appearance.colors.colLayer2
    radius: Appearance.rounding.small
    border.width: isCompleted ? 1 : 0
    border.color: Appearance.colors.colOutlineVariant

    HoverHandler {
        onHoveredChanged: root.hovered = hovered
    }

    Popup {
        id: confirmDeletePopup
        anchors.centerIn: parent
        implicitWidth: 280
        implicitHeight: deleteColumn.implicitHeight + 32
        modal: true
        focus: true
        background: Rectangle {
            color: Appearance.colors.colLayer3
            radius: Appearance.rounding.normal
            border.width: 1
            border.color: Appearance.colors.colOutlineVariant
        }

        ColumnLayout {
            id: deleteColumn
            anchors.fill: parent
            anchors.margins: 16
            spacing: 12

            StyledText {
                text: "Remove download?"
                font.bold: true
                color: Appearance.colors.colOnLayer0
                Layout.fillWidth: true
            }
            StyledText {
                text: root.filename
                color: Appearance.colors.colSubtext
                font.pixelSize: Appearance.font.pixelSize.small
                elide: Text.ElideRight
                Layout.fillWidth: true
            }
            RowLayout {
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignRight
                spacing: 10

                RippleButton {
                    implicitWidth: 80
                    implicitHeight: 36
                    buttonText: "Cancel"
                    onClicked: confirmDeletePopup.close()
                }
                RippleButton {
                    implicitWidth: 80
                    implicitHeight: 36
                    colBackground: Appearance.colors.colError
                    contentItem: StyledText {
                        text: "Remove"
                        color: Appearance.colors.colOnError
                        anchors.centerIn: parent
                    }
                    onClicked: {
                        Quickshell.execDetached(["hyprfetch", "remove", root.downloadId])
                        confirmDeletePopup.close()
                    }
                }
            }
        }
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 12
        spacing: 6

        RowLayout {
            Layout.fillWidth: true
            spacing: 8

            MaterialSymbol {
                text: root.isCompleted ? "check_circle" : "downloading"
                color: root.isCompleted ? Appearance.colors.colPrimary : Appearance.colors.colOnLayer0
                iconSize: 20
            }

            StyledText {
                text: root.filename
                font.bold: true
                color: Appearance.colors.colOnLayer0
                Layout.fillWidth: true
                elide: Text.ElideRight
            }

            StyledText {
                visible: !root.isCompleted
                text: root.speed + " • " + root.eta
                color: Appearance.colors.colSubtext
                font.pixelSize: Appearance.font.pixelSize.small
            }

            StyledText {
                visible: root.isCompleted
                text: "Completed"
                color: Appearance.colors.colPrimary
                font.pixelSize: Appearance.font.pixelSize.small
                font.bold: true
            }
        }

        ProgressBar {
            visible: !root.isCompleted
            value: root.progress
            Layout.fillWidth: true
            implicitHeight: 6
        }

        RowLayout {
            visible: root.isCompleted
            Layout.fillWidth: true
            Layout.alignment: Qt.AlignRight
            spacing: 8

            RippleButton {
                implicitWidth: 32
                implicitHeight: 32
                buttonRadius: Appearance.rounding.full
                colBackground: Appearance.colors.colLayer3
                contentItem: MaterialSymbol {
                    text: "open_in_new"
                    anchors.centerIn: parent
                    iconSize: 18
                    color: Appearance.colors.colOnLayer0
                }
                onClicked: {
                    if (root.filePath !== "") Quickshell.execDetached(["xdg-open", root.filePath])
                }
                StyledToolTip { text: "Open File" }
            }

            RippleButton {
                implicitWidth: 32
                implicitHeight: 32
                buttonRadius: Appearance.rounding.full
                colBackground: Appearance.colors.colLayer3
                contentItem: MaterialSymbol {
                    text: "folder_open"
                    anchors.centerIn: parent
                    iconSize: 18
                    color: Appearance.colors.colOnLayer0
                }
                onClicked: {
                    if (root.filePath !== "") {
                        let folder = root.filePath.substring(0, root.filePath.lastIndexOf('/'));
                        Quickshell.execDetached(["xdg-open", folder])
                    }
                }
                StyledToolTip { text: "Open Location" }
            }

            RippleButton {
                implicitWidth: 32
                implicitHeight: 32
                buttonRadius: Appearance.rounding.full
                colBackground: Appearance.colors.colLayer3
                contentItem: MaterialSymbol {
                    text: "delete"
                    anchors.centerIn: parent
                    iconSize: 18
                    color: Appearance.colors.colError
                }
                onClicked: confirmDeletePopup.open()
                StyledToolTip { text: "Remove from list" }
            }
        }
    }
}
