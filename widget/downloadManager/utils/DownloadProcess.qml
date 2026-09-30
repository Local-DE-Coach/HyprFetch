import QtQuick
import Quickshell
import Quickshell.Io

/**
 * Utility — runs the backend CLI (`hyprfetch add <url>` / `hyprfetch
 * reveal <id>`) as a SHORT-LIVED child process. No daemons, no polling:
 * the process starts on demand, exits, and only its exit code + last
 * stderr line are surfaced through `finished(ok, output)`.
 */
Process {
    id: proc

    /// Last stderr line (the CLI writes human-readable errors there).
    property string lastError: ""

    /// `ok` = exit code 0; `output` = last stderr line on failure.
    signal finished(bool ok, string output)

    command: []

    // Only stderr matters (the JSON/API surface is the status file);
    // SplitParser keeps just the last line — no unbounded buffering.
    stderr: SplitParser {
        onRead: data => {
            proc.lastError = String(data).trim();
        }
    }

    onExited: {
        const ok = proc.exitCode === 0;
        proc.finished(ok, ok ? "" : proc.lastError);
        proc.lastError = "";
    }

    /// Start a generic hyprfetch command, e.g. ["hyprfetch", "reveal", id].
    function runCommand(args) {
        proc.lastError = "";
        proc.command = args;
        proc.running = true;
    }

    /// Queue one URL on the daemon (`hyprfetch add <url>`).
    function addToQueue(url) {
        proc.runCommand(["hyprfetch", "add", url]);
    }
}
