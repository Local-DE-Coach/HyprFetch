import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Quickshell
import qs.modules.common
import qs.modules.common.widgets

Rectangle {
    id: root
    implicitHeight: 60
    color: Appearance.colors.colLayer1
    radius: Appearance.rounding.normal

    // Full-window overlay: dim + "confirm download path" dialog before the
    // download starts. Parented to the Qt Quick Controls overlay so it
    // covers the whole shell window regardless of where the sidebar is.
    Popup {
        id: filenamePopup
        parent: Overlay.overlay
        anchors.centerIn: parent
        width: parent ? parent.width : 0
        height: parent ? parent.height : 0
        padding: 0
        visible: false
        background: null

        contentItem: Item {
            anchors.fill: parent

            Rectangle {
                anchors.fill: parent
                color: "#CC000000"

                MouseArea {
                    anchors.fill: parent
                    onClicked: filenamePopup.close()
                }
            }

            Rectangle {
                anchors.centerIn: parent
                width: 400
                height: filenameColumn.implicitHeight + 40
                color: Appearance.colors.colLayer3
                radius: Appearance.rounding.normal
                border.width: 1
                border.color: Appearance.colors.colOutlineVariant

                MouseArea { anchors.fill: parent }

                ColumnLayout {
                    id: filenameColumn
                    anchors.fill: parent
                    anchors.margins: 20
                    spacing: 12

                    StyledText {
                        text: "Confirm Download Path"
                        font.bold: true
                        font.pixelSize: Appearance.font.pixelSize.large
                        color: Appearance.colors.colOnLayer0
                        Layout.fillWidth: true
                    }

                    StyledText {
                        text: "You can edit the full path below before downloading:"
                        color: Appearance.colors.colSubtext
                        font.pixelSize: Appearance.font.pixelSize.small
                        Layout.fillWidth: true
                    }

                    TextField {
                        id: pathInput
                        Layout.fillWidth: true
                        implicitHeight: 42
                        placeholderText: "/home/user/Downloads/..."
                        color: Appearance.colors.colOnLayer0
                        background: Rectangle {
                            color: Appearance.colors.colLayer2
                            radius: 8
                        }
                    }

                    RowLayout {
                        Layout.fillWidth: true
                        Layout.topMargin: 10
                        spacing: 10

                        Item { Layout.fillWidth: true }

                        RippleButton {
                            implicitWidth: 90
                            implicitHeight: 38
                            buttonRadius: Appearance.rounding.small
                            colBackground: Appearance.colors.colLayer2
                            contentItem: StyledText {
                                text: "Cancel"
                                color: Appearance.colors.colOnLayer0
                                anchors.centerIn: parent
                            }
                            onClicked: filenamePopup.close()
                        }

                        RippleButton {
                            implicitWidth: 110
                            implicitHeight: 38
                            buttonRadius: Appearance.rounding.small
                            colBackground: Appearance.colors.colPrimary
                            contentItem: StyledText {
                                text: "Download"
                                color: Appearance.colors.colOnPrimary
                                anchors.centerIn: parent
                                font.bold: true
                            }
                            onClicked: {
                                let url = urlInput.text
                                let finalPath = pathInput.text
                                Quickshell.execDetached(["hyprfetch", "add", url, "-o", finalPath])
                                filenamePopup.close()
                                urlInput.text = ""
                            }
                        }
                    }
                }
            }
        }

        onOpened: {
            pathInput.selectAll()
            pathInput.forceActiveFocus()
        }
    }

    RowLayout {
        anchors.fill: parent
        anchors.margins: 10
        spacing: 10

        TextField {
            id: urlInput
            Layout.fillWidth: true
            implicitHeight: 40
            placeholderText: "Enter URL to download..."
            color: Appearance.colors.colOnLayer0
            background: Rectangle {
                color: Appearance.colors.colLayer2
                radius: Appearance.rounding.small
            }
        }

        RippleButton {
            implicitWidth: 80
            implicitHeight: 40
            buttonRadius: Appearance.rounding.small
            colBackground: Appearance.colors.colPrimary

            contentItem: StyledText {
                text: "Start"
                color: Appearance.colors.colOnPrimary
                anchors.centerIn: parent
                font.bold: true
            }

            onClicked: {
                if (urlInput.text !== "") {
                    let defaultName = urlInput.text.split('/').pop().split('?')[0] || "download"
                    pathInput.text = Quickshell.env("HOME") + "/Downloads/" + defaultName
                    filenamePopup.open()
                }
            }
        }
    }
}
