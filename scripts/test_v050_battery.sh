#!/usr/bin/env bash
# ============================================================================
# v0.5.0 battery — the widget feature end-to-end.
#
# Boots the REAL release binary against a local mock update channel and a
# fake illogical-impulse quickshell config, then proves:
#
#   1. GET  /api/widget/status reports the environment correctly.
#   2. POST /api/widget/install downloads widget.tar.gz from the channel,
#      installs the module and WIRES THE BAR (import + DownloadWidget block
#      right after the RightToLeft anchor), byte-identical to what the
#      POSIX installer produces.
#   3. Re-install is idempotent (no duplicated blocks/imports).
#   4. POST /api/widget/uninstall removes everything and restores the bar
#      file exactly.
#   5. Install WITHOUT an ii config fails cleanly (no junk dirs).
#   6. The POSIX installer and the API produce IDENTICAL bar edits.
#
# Usage:  scripts/test_v050_battery.sh
# ============================================================================
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release/hyprfetch"
WIDGET_DIR="$ROOT/widget"
# Collision-proof ports: random base each run (20000–29999) unless pinned.
PORT="${PORT:-$(( (RANDOM % 3000) + 20000 ))}"
BASE_PORT=$((PORT+2))   # daemon HTTP port — never the default 7780, never collides with the channel

PASS=0; FAIL=0
check() {
  if [ "$2" = 0 ]; then PASS=$((PASS+1)); echo "  ✓ $1";
  else FAIL=$((FAIL+1)); echo "  ✗ FAIL: $1"; fi
}
wait_http() {
  local tries="${2:-20}"; for _ in $(seq 1 "$tries"); do
    curl -sf "$1" >/dev/null 2>&1 && return 0; sleep 0.4
  done; return 1
}

command -v curl >/dev/null 2>&1 || { echo "need curl"; exit 1; }
[ -x "$BIN" ] || { echo "release binary missing — run: cargo build --release"; exit 1; }

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"; [ -n "${SRV_PID:-}" ] && kill $SRV_PID 2>/dev/null; [ -n "${DAEMON_PID:-}" ] && { kill $DAEMON_PID 2>/dev/null; sleep 0.4; kill -9 $DAEMON_PID 2>/dev/null; }' EXIT

echo "=== mock channel: widget.tar.gz (packaged exactly like release.yml) ==="
CHANNEL_DIR="$WORK/channel"
mkdir -p "$CHANNEL_DIR"
tar czf "$CHANNEL_DIR/widget.tar.gz" -C "$WIDGET_DIR" install.sh README.md downloadManager
# publish the installer the way the Docs mirror does
tar xzf "$CHANNEL_DIR/widget.tar.gz" -O install.sh > "$CHANNEL_DIR/widget-install.sh"
sh -n "$CHANNEL_DIR/widget-install.sh"
check "published widget-install.sh passes sh -n" $?

python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$CHANNEL_DIR" >/dev/null 2>&1 &
SRV_PID=$!
wait_http "http://127.0.0.1:$PORT/widget.tar.gz" && check "mock channel is up" 0 || check "mock channel is up" 1

echo "=== fake ii environment ==="
HOME_DIR="$WORK/home"
QSROOT="$HOME_DIR/.config/quickshell/ii"
mkdir -p "$QSROOT/modules/ii/bar"
cat > "$QSROOT/modules/ii/bar/BarContent.qml" <<'QML'
import QtQuick
import QtQuick.Layouts
import qs
import qs.services
import qs.modules.common

Item {
    id: root

    RowLayout {
        id: rightSectionRowLayout
        anchors.fill: parent
        spacing: 5
        layoutDirection: Qt.RightToLeft

        RippleButton {
            id: rightSidebarButton
            Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
        }
    }
}
QML
cp "$QSROOT/modules/ii/bar/BarContent.qml" "$WORK/bar-original.qml"
mkdir -p "$HOME_DIR/.local/bin"
cp "$BIN" "$HOME_DIR/.local/bin/hyprfetch"
# dependency stubs so status reports are realistic
mkdir -p "$WORK/bin"
printf '#!/bin/sh\nexit 0\n' > "$WORK/bin/quickshell"; chmod 755 "$WORK/bin/quickshell"

echo "=== boot the real daemon (dedicated port) ==="
env -i HOME="$HOME_DIR" PATH="$WORK/bin:/usr/bin:/bin" LANG=C.UTF-8 \
  HYPRFETCH_UPDATE_CHANNEL="http://127.0.0.1:$PORT" \
  HYPRFETCH_QS_ROOT="$QSROOT" \
  "$HOME_DIR/.local/bin/hyprfetch" daemon start --bind "127.0.0.1:$BASE_PORT" >/dev/null 2>&1
check "daemon started" $?
DAEMON_PID=$(sed -n 's/.*"pid": *"\{0,1\}\([0-9]*\)"\{0,1\}.*/\1/p' "$HOME_DIR/.local/state/hyprfetch/hyprfetch.pid" 2>/dev/null | head -1)

BASE="http://127.0.0.1:$BASE_PORT"
wait_http "$BASE/healthz" && check "daemon answers" 0 || check "daemon answers" 1

echo "=== 1. widget status before install ==="
ST=$(curl -sf "$BASE/api/widget/status")
echo "$ST" | python3 -c '
import json,sys; d=json.load(sys.stdin)
assert d["qs_found"] is True, d
assert d["installed"] is False, d
assert d["integrated"] is False, d
assert d["quickshell_found"] is True, d
' && check "status: env found, not installed" 0 || check "status: env found, not installed" 1

echo "=== 2. install via the API ==="
ST=$(curl -sf -X POST "$BASE/api/widget/install")
echo "$ST" | python3 -c '
import json,sys; d=json.load(sys.stdin)
assert d.get("installed") is True, d
assert d.get("integrated") is True, d
assert not d.get("note"), d
' && check "install reports installed+integrated" 0 || check "install reports installed+integrated" 1
MOD="$QSROOT/modules/downloadManager"
[ -f "$MOD/DownloadWidget.qml" ] && check "module files installed" 0 || check "module files installed" 1
for f in components/RecentPopup.qml components/InputPopup.qml components/ActivePopup.qml components/CompletionToast.qml utils/DownloadProcess.qml; do
  [ -f "$MOD/$f" ] && check "file $f" 0 || check "file $f" 1
done
BAR="$QSROOT/modules/ii/bar/BarContent.qml"
grep -q "^import qs.modules.downloadManager$" "$BAR" && check "bar import added" 0 || check "bar import added" 1
grep -q "// HyprFetch download widget (auto-added" "$BAR" && check "bar block added" 0 || check "bar block added" 1
grep -c "DownloadWidget {" "$BAR" | grep -q '^1$' && check "single instance" 0 || check "single instance" 1
[ -f "$BAR.bak-hyprfetch" ] && check "backup kept" 0 || check "backup kept" 1

echo "=== 3. idempotent reinstall ==="
curl -sf -X POST "$BASE/api/widget/install" >/dev/null
grep -c "^import qs.modules.downloadManager$" "$BAR" | grep -q '^1$' && check "no duplicate import" 0 || check "no duplicate import" 1
grep -c "// HyprFetch download widget (auto-added" "$BAR" | grep -q '^1$' && check "no duplicate block" 0 || check "no duplicate block" 1

echo "=== 4. status after install ==="
ST=$(curl -sf "$BASE/api/widget/status")
echo "$ST" | python3 -c '
import json,sys; d=json.load(sys.stdin)
assert d["installed"] is True, d
assert d["integrated"] is True, d
assert d["up_to_date"] is True, d
assert d["version"], d
' && check "status: installed, integrated, up_to_date" 0 || check "status: installed, integrated, up_to_date" 1

echo "=== 5. uninstall restores everything ==="
ST=$(curl -sf -X POST "$BASE/api/widget/uninstall")
echo "$ST" | python3 -c '
import json,sys; d=json.load(sys.stdin)
assert d.get("installed") is False, d
assert d.get("bar_reverted") is True, d
' && check "uninstall reports clean" 0 || check "uninstall reports clean" 1
[ ! -d "$MOD" ] && check "module dir removed" 0 || check "module dir removed" 1
diff -q "$WORK/bar-original.qml" "$BAR" >/dev/null && check "bar file restored byte-identical" 0 || check "bar file restored byte-identical" 1

echo "=== 6. installer and API produce IDENTICAL bar edits ==="
curl -sf -X POST "$BASE/api/widget/install" >/dev/null
cp "$BAR" "$WORK/bar-via-api.qml"
cp "$WORK/bar-original.qml" "$BAR"
( cd "$WORK" && curl -fsSL "http://127.0.0.1:$PORT/widget-install.sh" \
    | HYPRFETCH_WIDGET_URL="http://127.0.0.1:$PORT/widget.tar.gz" HYPRFETCH_QS_ROOT="$QSROOT" PATH="$WORK/bin:$HOME_DIR/.local/bin:/usr/bin:/bin" sh >/dev/null 2>&1 )
diff -q "$WORK/bar-via-api.qml" "$BAR" >/dev/null && check "API edit == installer edit" 0 || check "API edit == installer edit" 1

echo "=== 7. install without ii config fails cleanly (second instance) ==="
NOII_ROOT="$WORK/no-ii"
NOII_PORT=$((BASE_PORT+1))
env -i HOME="$HOME_DIR" PATH="$WORK/bin:/usr/bin:/bin" LANG=C.UTF-8 \
  HYPRFETCH_UPDATE_CHANNEL="http://127.0.0.1:$PORT" \
  HYPRFETCH_QS_ROOT="$NOII_ROOT" \
  "$HOME_DIR/.local/bin/hyprfetch" serve --bind "127.0.0.1:$NOII_PORT" >/dev/null 2>&1 &
NOII_PID=$!
wait_http "http://127.0.0.1:$NOII_PORT/healthz" && check "second instance up" 0 || check "second instance up" 1
CODE=$(curl -s -o "$WORK/noii-body.json" -w "%{http_code}" -X POST "http://127.0.0.1:$NOII_PORT/api/widget/install")
[ "$CODE" = "500" ] && check "missing ii root -> HTTP 500" 0 || check "missing ii root -> HTTP 500 (got $CODE)" 1
grep -q "illogical-impulse quickshell config not found" "$WORK/noii-body.json" && check "error message is clear" 0 || check "error message is clear" 1
[ ! -d "$NOII_ROOT" ] && check "no junk dirs created" 0 || check "no junk dirs created" 1
kill $NOII_PID 2>/dev/null

echo ""
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" = "0" ]
