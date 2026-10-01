pragma Singleton
import QtQuick
import QtQml
QtObject {
    property QtObject colors: QtObject {
        property color colLayer0: "#ffffff"
        property color colLayer0Hover: "#f5f5f5"
        property color colLayer0Active: "#eeeeee"
        property color colOnLayer0: "#1b1b1b"
        property color colLayer1: "#f2f2f2"
        property color colOnLayer1: "#333333"
        property color colLayer2: "#e8e8e8"
        property color colLayer2Hover: "#dddddd"
        property color colOnLayer2: "#222222"
        property color colLayer3: "#d8d8d8"
        property color colSubtext: "#777777"
        property color colPrimary: "#65558f"
        property color colPrimaryHover: "#77699c"
        property color colOnPrimary: "#ffffff"
        property color colError: "#b3261e"
        property color colOnError: "#ffffff"
        property color colOutlineVariant: "#cccccc"
    }
    property QtObject rounding: QtObject {
        property int small: 12
        property int normal: 17
        property int full: 9999
    }
    property QtObject font: QtObject {
        property QtObject pixelSize: QtObject {
            property int small: 15
            property int normal: 16
            property int large: 17
        }
    }
}
