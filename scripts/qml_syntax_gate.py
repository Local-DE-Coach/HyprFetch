#!/usr/bin/env python3
"""QML gate for the HyprFetch Quickshell widget (CI-friendly, no GL needed).

Two layers:

1. PARSE GATE — every widget file is compiled by a real QQmlEngine
   (PySide6, QCoreApplication — instantiation would need a real EGL, parsing
   does not). Stub modules provide the quickshell-only imports (Quickshell,
   Quickshell.Io) AND the ii-only imports (qs.modules.common → Appearance,
   qs.modules.common.widgets → StyledText/RippleButton/MaterialSymbol).
   Catches: syntax errors, unknown types, bad imports, duplicate ids,
   malformed signal handlers, broken grouped properties — everything up to
   binding evaluation (and binding evaluation itself when EGL exists).

2. STATIC CROSS-REFERENCE CHECK — what parse-only cannot see:
   * `Appearance.colors.<role>` references vs the real ii role manifest
     (verified against end-4's Appearance.qml)
   * `Appearance.rounding.<x>` / `Appearance.font.pixelSize.<x>` refs
   * relative component references (DownloadManager ↔ components/)

Exit 0 = pass. Run: python3 scripts/qml_syntax_gate.py
"""

import os
import re
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WIDGET = os.path.join(REPO, "widget", "downloadManager")
STUBS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "qml_stubs")

STUB_FILES = {
    "Quickshell/qmldir": """module Quickshell
singleton Quickshell 1.0 QuickshellSingleton.qml
""",
    "Quickshell/QuickshellSingleton.qml": """pragma Singleton
import QtQml
QtObject {
    function env(name) { return ""; }
    function execDetached(cmd) { return true; }
}
""",
    "Quickshell/Io/qmldir": """module Quickshell.Io
FileView 1.0 FileView.qml
Process 1.0 Process.qml
SplitParser 1.0 SplitParser.qml
StdioCollector 1.0 StdioCollector.qml
""",
    "Quickshell/Io/FileView.qml": """import QtQml
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
""",
    "Quickshell/Io/Process.qml": """import QtQml
QtObject {
    property var command: []
    property bool running: false
    property int exitCode: 0
    property QtObject stdout
    property QtObject stderr
    signal exited()
}
""",
    "Quickshell/Io/SplitParser.qml": """import QtQml
QtObject {
    signal read(string data)
}
""",
    "Quickshell/Io/StdioCollector.qml": """import QtQml
QtObject {
    property string text
    signal streamFinished()
}
""",
    # qs.modules.common — ii's core singletons as used by the widget
    "qs/modules/common/qmldir": """module qs.modules.common
singleton Appearance 1.0 Appearance.qml
singleton Config 1.0 Config.qml
""",
    "qs/modules/common/Appearance.qml": """pragma Singleton
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
""",
    "qs/modules/common/Config.qml": """pragma Singleton
import QtQml
QtObject {
    property var options: ({})
}
""",
    # qs.modules.common.widgets — ii's styled building blocks
    "qs/modules/common/widgets/qmldir": """module qs.modules.common.widgets
StyledText 1.0 StyledText.qml
RippleButton 1.0 RippleButton.qml
MaterialSymbol 1.0 MaterialSymbol.qml
StyledToolTip 1.0 StyledToolTip.qml
""",
    "qs/modules/common/widgets/StyledText.qml": """import QtQuick
Text {
    color: "#1b1b1b"
}
""",
    "qs/modules/common/widgets/MaterialSymbol.qml": """import QtQuick
Text {
    property real iconSize: 16
    font.pixelSize: iconSize
}
""",
    "qs/modules/common/widgets/RippleButton.qml": """import QtQuick
import QtQuick.Controls
Control {
    property string buttonText: ""
    property real buttonRadius: 4
    property color colBackground: "transparent"
    signal clicked()
    background: Rectangle { color: colBackground; radius: buttonRadius }
    contentItem: Text { text: buttonText }
}
""",
    "qs/modules/common/widgets/StyledToolTip.qml": """import QtQuick
import QtQuick.Controls
ToolTip {
    property bool extraVisibleCondition: true
}
""",
}

# Appearance roles used by the widget — must all exist on end-4's
# illogical-impulse Appearance.colors (verified against upstream main).
APPEARANCE_ROLES = {
    "colLayer0", "colLayer0Hover", "colLayer0Active", "colOnLayer0",
    "colLayer1", "colOnLayer1", "colLayer2", "colLayer2Hover", "colOnLayer2",
    "colLayer3", "colSubtext",
    "colPrimary", "colPrimaryHover", "colOnPrimary",
    "colError", "colOnError", "colOutlineVariant",
}

ROUNDING = {"small", "normal", "full"}
PIXEL_SIZES = {"small", "normal", "large"}

WIDGET_FILES = [
    ("DownloadManager.qml", False),
    ("components/DownloadHeader.qml", False),
    ("components/DownloadList.qml", False),
    ("components/DownloadItem.qml", False),
    ("components/DownloadInputBar.qml", False),
]


def write_stubs():
    if os.path.exists(STUBS):
        import shutil
        shutil.rmtree(STUBS)
    for rel, body in STUB_FILES.items():
        path = os.path.join(STUBS, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            f.write(body)


def strip_strings_and_comments(src: str) -> str:
    """Single pass: blank out comments AND string contents (strings keep
    their quotes) so bracket balance is meaningful."""
    out = []
    i, n = 0, len(src)
    in_str = None
    while i < n:
        ch = src[i]
        if in_str:
            if ch == "\\":
                i += 2
                continue
            if ch == in_str:
                in_str = None
                out.append(ch)
            i += 1
            continue
        if ch in "\"'":
            in_str = ch
            out.append(ch)
            i += 1
            continue
        if ch == "/" and i + 1 < n and src[i + 1] == "/":
            while i < n and src[i] != "\n":
                i += 1
            continue
        if ch == "/" and i + 1 < n and src[i + 1] == "*":
            i += 2
            while i + 1 < n and not (src[i] == "*" and src[i + 1] == "/"):
                i += 1
            i += 2
            continue
        out.append(ch)
        i += 1
    return "".join(out)


def balanced(clean: str) -> str:
    depth = {"{": 0, "(": 0, "[": 0}
    for ch in clean:
        if ch in depth:
            depth[ch] += 1
        elif ch in ")}]":
            pair = {"}": "{", ")": "(", "]": "["}[ch]
            depth[pair] -= 1
            if depth[pair] < 0:
                return f"unbalanced closing '{ch}'"
    for k, v in depth.items():
        if v != 0:
            return f"unbalanced '{k}' (depth {v})"
    return ""


def check_cross_refs() -> list:
    problems = []
    sources = {}
    for rel, _ in WIDGET_FILES:
        path = os.path.join(WIDGET, rel)
        sources[rel] = strip_strings_and_comments(open(path, encoding="utf-8").read())

    used_roles = set()
    used_rounding = set()
    used_sizes = set()
    for src in sources.values():
        used_roles |= set(re.findall(r"Appearance\.colors\.(\w+)", src))
        used_rounding |= set(re.findall(r"Appearance\.rounding\.(\w+)", src))
        used_sizes |= set(re.findall(r"Appearance\.font\.pixelSize\.(\w+)", src))
    bad = used_roles - APPEARANCE_ROLES
    if bad:
        problems.append(f"Appearance.colors roles not in ii manifest: {sorted(bad)}")
    bad = used_rounding - ROUNDING
    if bad:
        problems.append(f"Appearance.rounding values not in ii manifest: {sorted(bad)}")
    bad = used_sizes - PIXEL_SIZES
    if bad:
        problems.append(f"Appearance.font.pixelSize values not in ii manifest: {sorted(bad)}")

    # Cross-file component references: everything DownloadManager.qml
    # instantiates must exist next to it (components/), and the delegate
    # type used by DownloadList too.
    refs = set(re.findall(r"\b(Download\w+)\s*[{]", sources["DownloadManager.qml"]))
    for r in sorted(refs):
        if not os.path.isfile(os.path.join(WIDGET, "components", f"{r}.qml")):
            problems.append(f"DownloadManager.qml instantiates {r} — components/{r}.qml missing")
    delegate = set(re.findall(r"delegate:\s*(\w+)", sources["components/DownloadList.qml"]))
    for d in sorted(delegate):
        if not os.path.isfile(os.path.join(WIDGET, "components", f"{d}.qml")):
            problems.append(f"DownloadList delegate {d} — components/{d}.qml missing")

    # The legacy bar-widget API must be gone for good.
    for name, src in sources.items():
        if "MaterialTheme." in src:
            problems.append(f"{name}: still references MaterialTheme (pre-v0.5.1 API)")

    return problems


def parse_gate() -> bool:
    use_gui = False
    try:
        from PySide6.QtGui import QGuiApplication  # noqa: F401
        use_gui = True
    except Exception:
        pass

    if use_gui:
        from PySide6.QtGui import QGuiApplication
        app = QGuiApplication.instance() or QGuiApplication([])  # noqa: F841
    else:
        from PySide6.QtCore import QCoreApplication
        app = QCoreApplication.instance() or QCoreApplication([])  # noqa: F841
    from PySide6.QtCore import QUrl
    from PySide6.QtQml import QQmlComponent, QQmlEngine

    engine = QQmlEngine()
    engine.addImportPath(STUBS)

    collected = []

    def collect(msgs):
        for m in msgs:
            try:
                collected.append(m.toString())
            except Exception:
                collected.append(str(m))

    engine.warnings.connect(collect)

    failed = False

    def report(rel, problems):
        nonlocal failed
        # Whitelist = stub-land artifacts that CANNOT exist against real
        # quickshell/ii (Overlay parent, singleton lookups without a shell).
        patterns = ("Overlay", "QsWindow")
        real = [p for p in problems if not any(w in p for w in patterns)]
        if real:
            failed = True
            print(f"FAIL {rel}:")
            for p in real:
                print(f"   {p}")
        elif problems:
            print(f"PASS {rel} (only whitelisted stub-land notices)")
        else:
            print(f"PASS {rel}")

    for rel, _ in WIDGET_FILES:
        path = os.path.join(WIDGET, rel)
        src = open(path, encoding="utf-8").read()
        bal = balanced(strip_strings_and_comments(src))
        if bal:
            print(f"FAIL {rel}: {bal}")
            failed = True
            continue
        comp = QQmlComponent(engine, QUrl.fromLocalFile(path))
        problems = [f"{e.description()} (line {e.line()})" for e in comp.errors()]
        if not problems and use_gui:
            # Instantiate to surface binding/property errors for real.
            collected.clear()
            obj = comp.create()
            if obj is None:
                problems += [f"{e.description()} (line {e.line()})" for e in comp.errors()]
            else:
                problems += list(collected)
                obj.deleteLater()
        report(rel, problems)

    if not use_gui:
        print("(env lacks EGL — instantiation skipped, parse-only gate)")
    return not failed


def main() -> int:
    write_stubs()
    os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

    ok = parse_gate()
    for problem in check_cross_refs():
        ok = False
        print(f"FAIL xref: {problem}")

    if not ok:
        print("\nQML gate: FAIL")
        return 1
    print("\nQML gate: all widget files compile + cross-references consistent")
    return 0


if __name__ == "__main__":
    sys.exit(main())
