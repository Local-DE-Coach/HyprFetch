import QtQuick
import QtQuick.Controls
Control {
    property string buttonText: ""
    property real buttonRadius: 4
    property color colBackground: "transparent"
    signal clicked()
    background: Rectangle { color: colBackground; radius: buttonRadius }
    contentItem: Text { text: buttonText }
}
