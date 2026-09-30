import QtQuick
Item {
    default property alias contentData: holder.data
    property PopupAnchor anchor: anchorObj
    property color color: "transparent"
    PopupAnchor { id: anchorObj }
    Item { id: holder; visible: false }
}
