import QtQml
QtObject {
    property var command: []
    property bool running: false
    property int exitCode: 0
    property QtObject stdout
    property QtObject stderr
    signal exited()
}
