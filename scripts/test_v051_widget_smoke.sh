#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# Smoke test for widget/install.sh (v0.5.1 sidebar widget).
# Runs the installer against a FAKE ii checkout — asserts every integration
# step, idempotency, and a byte-exact uninstall restore. Never touches $HOME.
# ---------------------------------------------------------------------------
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
UPSTREAM="$(dirname "$0")/fixtures/SidebarLeftContent.upstream.qml"
[ -f "$UPSTREAM" ] || { echo "missing fixture: $UPSTREAM" >&2; exit 1; }

export HYPRFETCH_QS_ROOT="$T/ii"
export HYPRFETCH_II_CONFIG="$T/ii-config/config.json"
export XDG_DATA_HOME="$T/xdg"
SIDEBAR="$HYPRFETCH_QS_ROOT/modules/ii/sidebarLeft"

# --- fake ii checkout -------------------------------------------------------
mkdir -p "$SIDEBAR" "$HYPRFETCH_QS_ROOT/modules/ii/bar" "$T/ii-config" "$T/bin"
cp "$UPSTREAM" "$SIDEBAR/SidebarLeftContent.qml"
# a bar with the LEGACY v0.5.0 auto-edit applied (must be cleaned by install)
cat > "$HYPRFETCH_QS_ROOT/modules/ii/bar/BarContent.qml" <<'EOF'
import qs.modules.common

Item {
    RowLayout {
        layoutDirection: Qt.RightToLeft
        RippleButton { id: b }
import qs.modules.downloadManager
        // HyprFetch download widget (auto-added — delete this block to remove)
        DownloadWidget {
            Layout.alignment: Qt.AlignVCenter
        }
    }
}
EOF
mkdir -p "$HYPRFETCH_QS_ROOT/modules/downloadManager/components"
echo "old" > "$HYPRFETCH_QS_ROOT/modules/downloadManager/DownloadWidget.qml"
echo '{"policies":{"ai":1}}' > "$HYPRFETCH_II_CONFIG"

# --- stub binaries so dependency checks pass --------------------------------
for B in quickshell hyprfetch; do
  printf '#!/bin/sh\nexit 0\n' > "$T/bin/$B"
  chmod +x "$T/bin/$B"
done
export PATH="$T/bin:$PATH"

pass() { printf '  ✓ %s\n' "$1"; }
fail() { printf '  ✗ %s\n' "$1" >&2; exit 1; }

echo "== run 1 (LOCAL mode, from the repo widget/ dir) =="
( cd "$REPO/widget" && sh ./install.sh --no-restart ) > "$T/run1.log" 2>&1 || {
  cat "$T/run1.log" >&2; fail "installer exited non-zero"; }

grep -q "Installed widget files" "$T/run1.log" || { cat "$T/run1.log"; fail "no install log"; }
grep -q "Removed the old bar widget" "$T/run1.log" || fail "legacy dir not removed"
grep -q "Removed the old widget from your bar" "$T/run1.log" || fail "legacy bar edit not removed"
pass "legacy v0.5.0 bar widget cleaned"

[ -f "$HYPRFETCH_QS_ROOT/modules/ii/sidebarLeft/downloadManager/DownloadManager.qml" ] || fail "DownloadManager.qml missing"
for f in DownloadHeader DownloadList DownloadItem DownloadInputBar; do
  [ -f "$HYPRFETCH_QS_ROOT/modules/ii/sidebarLeft/downloadManager/components/$f.qml" ] || fail "$f.qml missing"
done
pass "all 5 widget files installed"

[ ! -e "$HYPRFETCH_QS_ROOT/modules/downloadManager" ] || fail "legacy dir still present"
grep -q "HyprFetch download widget" "$HYPRFETCH_QS_ROOT/modules/ii/bar/BarContent.qml" && fail "legacy bar edit still present" || true
grep -q "DownloadWidget" "$HYPRFETCH_QS_ROOT/modules/ii/bar/BarContent.qml" && fail "legacy bar block still present" || true
pass "legacy files gone"

grep -q 'import "./downloadManager"' "$SIDEBAR/SidebarLeftContent.qml" || fail "import line missing"
grep -q "property bool downloadManagerEnabled:" "$SIDEBAR/SidebarLeftContent.qml" || fail "policy prop missing"
grep -q '...(root.downloadManagerEnabled ? \[{"icon": "download"' "$SIDEBAR/SidebarLeftContent.qml" || fail "tab entry missing"
grep -q '...(root.downloadManagerEnabled ? \[downloadManager.createObject()\]' "$SIDEBAR/SidebarLeftContent.qml" || fail "children entry missing"
grep -q "Component { id: downloadManager; DownloadManager {} }" "$SIDEBAR/SidebarLeftContent.qml" || fail "component missing"
pass "all 5 sidebar edits present"

# QML syntax: the entry before the inserted tab MUST end with a comma now
# (upstream ii omits it — without the added comma the spread syntax is a
# parse error).
python3 - "$SIDEBAR/SidebarLeftContent.qml" <<'PY'
import sys
lines = open(sys.argv[1]).read().splitlines()
for i, l in enumerate(lines):
    if '...(root.downloadManagerEnabled ? [{"icon": "download"' in l:
        prev = lines[i - 1].rstrip()
        assert prev.endswith(','), f"line before Downloads tab entry lacks comma: {prev!r}"
        break
else:
    raise SystemExit("Downloads tab entry not found")
PY
pass "trailing comma added to the previous tab entry"

# braces still balanced in the edited QML
python3 - "$SIDEBAR/SidebarLeftContent.qml" <<'PY'
import sys
s = open(sys.argv[1]).read()
assert s.count('{') == s.count('}'), "brace mismatch after edit"
PY
pass "edited QML braces balanced"

python3 -c "import json,sys; c=json.load(open(sys.argv[1])); assert c['policies']['downloadManager']==1" "$HYPRFETCH_II_CONFIG" || fail "policy not set"
pass "config.json policy set"

[ -f "$T/xdg/download-manager/status.json" ] || fail "status.json not created"
pass "initial status.json created"
[ -f "$SIDEBAR/SidebarLeftContent.qml.bak-hyprfetch" ] || fail "sidebar backup missing"
pass "sidebar backup kept"

echo "== run 2 (idempotency) =="
( cd "$REPO/widget" && sh ./install.sh --no-restart ) > "$T/run2.log" 2>&1
grep -q "already present" "$T/run2.log" || fail "second run not idempotent"
pass "second run is a no-op"

echo "== run 3 (bootstrap mode via stdin pipe + file:// tarball) =="
tar czf "$T/widget.tar.gz" -C "$REPO/widget" install.sh README.md downloadManager
( mkdir -p "$T/cwd" && cd "$T/cwd" && \
  HYPRFETCH_WIDGET_URL="file://$T/widget.tar.gz" sh -s -- --no-restart \
    < "$REPO/widget/install.sh" ) > "$T/run3.log" 2>&1
grep -q "Downloading the latest widget" "$T/run3.log" || fail "bootstrap mode not triggered"
grep -q "Installed widget files" "$T/run3.log" || fail "bootstrap install failed"
pass "curl|sh bootstrap path works (file:// tarball)"

echo "== uninstall =="
cp "$UPSTREAM" "$T/expected-restore.qml"
( cd "$REPO/widget" && sh ./install.sh --uninstall --no-restart ) > "$T/un.log" 2>&1
[ ! -d "$HYPRFETCH_QS_ROOT/modules/ii/sidebarLeft/downloadManager" ] || fail "widget dir not removed"
diff -u "$T/expected-restore.qml" "$SIDEBAR/SidebarLeftContent.qml" > "$T/restore.diff" 2>&1 \
  || { cat "$T/restore.diff" >&2; fail "sidebar not restored byte-exact"; }
pass "uninstall restores SidebarLeftContent.qml byte-exact"

echo ""
echo "ALL SMOKE TESTS PASSED ✓"
