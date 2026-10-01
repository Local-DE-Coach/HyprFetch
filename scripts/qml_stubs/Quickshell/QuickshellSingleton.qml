pragma Singleton
import QtQml
QtObject {
    function env(name) { return ""; }
    function execDetached(cmd) { return true; }
}
