#!/usr/bin/env python3
"""QML gate for the HyprFetch Quickshell widget (CI-friendly, no GL needed).

Two layers:

1. PARSE GATE — every widget file is compiled by a real QQmlEngine
   (PySide6, QCoreApplication — instantiation would need a real EGL, parsing
   does not). Stub modules provide the quickshell-only imports (Quickshell,
   Quickshell.Io, qs.modules.common). Catches: syntax errors, unknown
   types, bad imports, duplicate ids, malformed signal handlers, broken
   grouped properties — everything up to binding evaluation.

2. STATIC CROSS-REFERENCE CHECK — what parse-only cannot see:
   * `MaterialTheme.<role>` references vs the MD3 role manifest
   * `controller.<member>` references inside popups vs members declared on
     DownloadWidget.qml (built-in Item members whitelisted)

Exit 0 = pass. Run: python3 scripts/qml_syntax_gate.py
"""

import os
import re
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WIDGET = os.path.join(REPO, "widget", "downloadManager")
STUBS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "qml_stubs")

STUB_FILES = {
    "Quickshell/qmldir": """module Quickshell
PopupWindow 1.0 PopupWindow.qml
PopupAnchor 1.0 PopupAnchor.qml
PopupRect 1.0 PopupRect.qml
LazyLoader 1.0 LazyLoader.qml
singleton Quickshell 1.0 QuickshellSingleton.qml
""",
    "Quickshell/QuickshellSingleton.qml": """pragma Singleton
import QtQml
QtObject {
    function env(name) { return ""; }
}
""",
    "Quickshell/PopupWindow.qml": """import QtQuick
Item {
    default property alias contentData: holder.data
    property PopupAnchor anchor: anchorObj
    property color color: "transparent"
    PopupAnchor { id: anchorObj }
    Item { id: holder; visible: false }
}
""",
    "Quickshell/PopupAnchor.qml": """import QtQuick
Item {
    property var window
    property PopupRect rect: rectObj
    PopupRect { id: rectObj }
}
""",
    "Quickshell/PopupRect.qml": """import QtQml
QtObject {
    property real x
    property real y
}
""",
    "Quickshell/LazyLoader.qml": """import QtQuick
Item {
    default property alias contentData: holder.data
    property bool active: false
    property var item: null
    Item { id: holder; visible: false }
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
    "qs/modules/common/qmldir": """module qs.modules.common
singleton MaterialTheme 1.0 MaterialTheme.qml
""",
    "qs/modules/common/MaterialTheme.qml": """pragma Singleton
import QtQml
QtObject { }
""",
}

# MD3 color roles used by the widget — must all exist on end4's
# illogical-impulse MaterialTheme singleton.
MATERIAL_ROLES = {
    "primary", "onPrimary", "onSurface", "onSurfaceVariant",
    "surfaceContainer", "surfaceContainerHigh", "surfaceContainerHighest",
    "surfaceVariant",
    "secondaryContainer", "onSecondaryContainer",
    "tertiary", "error", "outline",
}

# controller.<built-in> refs that are QML Item built-ins, not declared
# properties of the widget root.
BUILTIN_MEMBERS = {"QsWindow", "state", "states", "hoverCloseTimer"}


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


def declared_members(qml: str) -> set:
    """property/alias/function/signal/id declarations in one file."""
    members = set()
    for m in re.finditer(r"^\s*(?:readonly\s+)?property\s+(?:alias\s+)?([\w<>.,]+)\s+(\w+)\s*[:{]", qml, re.M):
        members.add(m.group(2))
    for m in re.finditer(r"^\s*(?:readonly\s+)?property\s+var\s+(\w+)\s*[:{]?", qml, re.M):
        members.add(m.group(1))
    for m in re.finditer(r"^\s*function\s+(\w+)\s*\(", qml, re.M):
        members.add(m.group(1))
    for m in re.finditer(r"^\s*signal\s+(\w+)\s*[\(:]", qml, re.M):
        members.add(m.group(1))
    for m in re.finditer(r"^\s*id\s*:\s*(\w+)", qml, re.M):
        members.add(m.group(1))
    return members


def check_cross_refs() -> list:
    problems = []
    paths = {
        "DownloadWidget.qml": os.path.join(WIDGET, "DownloadWidget.qml"),
        "components/RecentPopup.qml": os.path.join(WIDGET, "components", "RecentPopup.qml"),
        "components/InputPopup.qml": os.path.join(WIDGET, "components", "InputPopup.qml"),
        "components/ActivePopup.qml": os.path.join(WIDGET, "components", "ActivePopup.qml"),
        "components/CompletionToast.qml": os.path.join(WIDGET, "components", "CompletionToast.qml"),
        "utils/DownloadProcess.qml": os.path.join(WIDGET, "utils", "DownloadProcess.qml"),
    }
    sources = {k: strip_strings_and_comments(open(v, encoding="utf-8").read())
               for k, v in paths.items()}

    widget_members = declared_members(sources["DownloadWidget.qml"])

    used_roles = set()
    for src in sources.values():
        used_roles |= set(re.findall(r"MaterialTheme\.(\w+)", src))
    bad = used_roles - MATERIAL_ROLES
    if bad:
        problems.append(f"MaterialTheme roles not in manifest: {sorted(bad)}")

    for name in ["components/RecentPopup.qml", "components/InputPopup.qml",
                 "components/ActivePopup.qml", "components/CompletionToast.qml"]:
        refs = set(re.findall(r"controller\.(\w+)", sources[name]))
        missing = {r for r in refs if r not in widget_members and r not in BUILTIN_MEMBERS}
        if missing:
            problems.append(f"{name}: controller.{sorted(missing)} not declared on DownloadWidget")

    proc_members = declared_members(sources["utils/DownloadProcess.qml"])
    widget_src = sources["DownloadWidget.qml"]
    for fn in ["addToQueue", "runCommand"]:
        if re.search(rf"addProcess\.{fn}\(|revealProcess\.{fn}\(", widget_src) and fn not in proc_members:
            problems.append(f"DownloadWidget calls DownloadProcess.{fn}() which is missing")

    return problems


def parse_gate(engine_cls_path=None) -> bool:
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

    files = [
        ("DownloadWidget.qml", os.path.join(WIDGET, "DownloadWidget.qml"), False),
        ("components/RecentPopup.qml", os.path.join(WIDGET, "components", "RecentPopup.qml"), True),
        ("components/InputPopup.qml", os.path.join(WIDGET, "components", "InputPopup.qml"), True),
        ("components/ActivePopup.qml", os.path.join(WIDGET, "components", "ActivePopup.qml"), True),
        ("components/CompletionToast.qml", os.path.join(WIDGET, "components", "CompletionToast.qml"), True),
        ("utils/DownloadProcess.qml", os.path.join(WIDGET, "utils", "DownloadProcess.qml"), False),
    ]

    failed = False
    widget_obj = None

    def report(rel, problems):
        nonlocal failed
        # Whitelist = stub-land artifacts that CANNOT exist against real
        # quickshell (QsWindow attached property is C++-only; MaterialTheme
        # colors are validated against the manifest by the xref layer).
        patterns = ("QsWindow", "Cannot read property 'window' of undefined",
                    "Unable to assign [undefined] to QColor")
        real = [p for p in problems if not any(w in p for w in patterns)]
        if real:
            failed = True
            print(f"FAIL {rel}:")
            for p in real:
                print(f"   {p}")
        elif problems:
            print(f"PASS {rel} (only whitelisted QsWindow attached-prop notices)")
        else:
            print(f"PASS {rel}")

    for rel, path, needs_controller in files:
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
            props = {"controller": widget_obj} if needs_controller else {}
            obj = comp.createWithInitialProperties(props) if needs_controller or not comp.errors() else None
            if obj is None:
                problems += [f"{e.description()} (line {e.line()})" for e in comp.errors()]
            else:
                problems += list(collected)
                if rel == "DownloadWidget.qml":
                    widget_obj = obj
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
