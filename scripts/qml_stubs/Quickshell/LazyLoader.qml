import QtQuick
Item {
    default property alias contentData: holder.data
    property bool active: false
    property var item: null
    Item { id: holder; visible: false }
}
