import QtQml
QtObject {
    property string path
    property bool watchChanges: false
    property bool printErrors: false
    signal loaded()
    signal fileChanged()
    signal loadFailed(string err)
    function text() { return ""; }
    function reload() { }
}
