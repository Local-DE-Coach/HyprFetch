import QtQuick
import QtQuick.Layouts
import qs.modules.common
import qs.modules.common.widgets

Rectangle {
    id: root
    implicitHeight: 50
    color: Appearance.colors.colLayer0
    radius: Appearance.rounding.normal

    signal refreshRequested()

    RowLayout {
        anchors.fill: parent
        anchors.margins: 10
        spacing: 10

        StyledText {
            text: "Download Manager"
            font.bold: true
            font.pixelSize: Appearance.font.pixelSize.large
            color: Appearance.colors.colOnLayer0
            Layout.fillWidth: true
        }

        RippleButton {
            implicitWidth: 30
            implicitHeight: 30
            buttonRadius: Appearance.rounding.full
            colBackground: Appearance.colors.colLayer2

            contentItem: MaterialSymbol {
                text: "refresh"
                anchors.centerIn: parent
                iconSize: 18
                color: Appearance.colors.colOnLayer0
            }
            onClicked: root.refreshRequested()
        }
    }
}
