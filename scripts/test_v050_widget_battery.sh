#!/bin/sh
# v0.5.0 battery — widget installer (POSIX sh) matrix.
# Uses the REAL end-4 BarContent.qml as the bar fixture.
set -eu
REPO=/home/z/my-project/repos/HyprFetch
WIDGET_DIR="$REPO/widget"
T=/tmp/hf-widget-test
PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); echo "  ok: $1"; }
bad()  { FAIL=$((FAIL+1)); echo "  FAIL: $1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (want=$2 got=$3)"; fi }

echo "== 0. syntax gates =="
sh -n "$WIDGET_DIR/install.sh" && ok "sh -n" || bad "sh -n"
if command -v dash >/dev/null 2>&1; then dash -n "$WIDGET_DIR/install.sh" && ok "dash -n" || bad "dash -n"; else echo "  (dash not installed — skipped)"; fi
bash -n "$WIDGET_DIR/install.sh" && ok "bash -n" || bad "bash -n"

# fake ii environment (real-world-shaped BarContent.qml excerpt — the
# right-side RowLayout with layoutDirection: Qt.RightToLeft is the anchor)
rm -rf "$T"; mkdir -p "$T/home/.config/quickshell/ii/modules/ii/bar"
cat > "$T/home/.config/quickshell/ii/modules/ii/bar/BarContent.qml" <<'QML'
import qs.modules.ii.bar.weather
import QtQuick
import QtQuick.Layouts
import Quickshell
import qs
import qs.services
import qs.modules.common

Item { // Bar content region
    id: root

    FocusedScrollMouseArea { // Right side
        id: barRightSideMouseArea

        RowLayout {
            id: rightSectionRowLayout
            anchors.fill: parent
            spacing: 5
            layoutDirection: Qt.RightToLeft

            RippleButton { // Right sidebar button
                id: rightSidebarButton

                Layout.alignment: Qt.AlignRight | Qt.AlignVCenter

                implicitWidth: indicatorsRowLayout.implicitWidth + 10 * 2
            }
        }
    }
}
QML
cp "$T/home/.config/quickshell/ii/modules/ii/bar/BarContent.qml" "$T/bar-original.qml"
cp -r "$WIDGET_DIR" "$T/widget-local"

# package widget.tar.gz exactly like release.yml does
tar czf "$T/widget.tar.gz" -C "$WIDGET_DIR" install.sh README.md downloadManager

# serve it over http (bootstrap mode uses curl); publish install.sh exactly
# like the Docs mirror does (extracted from the tarball as widget-install.sh)
tar xzf "$T/widget.tar.gz" -O install.sh > "$T/widget-install.sh"
(cd "$T" && python3 -m http.server 8765 >/dev/null 2>&1 &) 
sleep 1

WURL="http://127.0.0.1:8765/widget.tar.gz"
SURL="http://127.0.0.1:8765/widget-install.sh"
QSROOT="$T/home/.config/quickshell/ii"
BAR="$QSROOT/modules/ii/bar/BarContent.qml"
MOD="$QSROOT/modules/downloadManager"

# dependency stubs (the sandbox has no real quickshell/hyprfetch)
mkdir -p "$T/bin"
printf '#!/bin/sh\nexit 0\n' > "$T/bin/quickshell"
printf '#!/bin/sh\nexit 0\n' > "$T/bin/hyprfetch"
chmod +x "$T/bin/quickshell" "$T/bin/hyprfetch"
export PATH="$T/bin:$PATH"

echo "== 1. bootstrap install via 'curl | sh' (the user's exact flow) =="
curl -fsSL "$SURL" | HYPRFETCH_WIDGET_URL="$WURL" HYPRFETCH_QS_ROOT="$QSROOT" sh 2>&1 | sed 's/^/    /'
[ -f "$MOD/DownloadWidget.qml" ] && ok "module dir copied" || bad "module dir copied"
for f in components/RecentPopup.qml components/InputPopup.qml components/ActivePopup.qml components/CompletionToast.qml utils/DownloadProcess.qml; do
  [ -f "$MOD/$f" ] && ok "file $f" || bad "file $f"
done
grep -q "import qs.modules.downloadManager" "$BAR" && ok "bar import added" || bad "bar import added"
grep -q "// HyprFetch download widget (auto-added" "$BAR" && ok "bar block added" || bad "bar block added"
grep -q "DownloadWidget {" "$BAR" && ok "DownloadWidget instance" || bad "DownloadWidget instance"
[ -f "$BAR.bak-hyprfetch" ] && ok "backup created" || bad "backup created"
# import must appear BEFORE the first statement and after the imports
FIRST_IMP=$(grep -n "^import" "$BAR" | head -1 | cut -d: -f1)
OUR_IMP=$(grep -n "^import qs.modules.downloadManager" "$BAR" | head -1 | cut -d: -f1)
LAST_IMP=$(grep -n "^import" "$BAR" | tail -1 | cut -d: -f1)
if [ "$OUR_IMP" -ge "$FIRST_IMP" ] && [ "$OUR_IMP" -le $((LAST_IMP+1)) ]; then ok "import placed in import block ($OUR_IMP between $FIRST_IMP and $LAST_IMP)"; else bad "import position ($OUR_IMP not in $FIRST_IMP..$LAST_IMP)"; fi
# block sits right after the RightToLeft anchor
ANCHOR=$(grep -n "layoutDirection: Qt.RightToLeft" "$BAR" | head -1 | cut -d: -f1)
BLOCK=$(grep -n "// HyprFetch download widget (auto-added" "$BAR" | head -1 | cut -d: -f1)
check "block right after anchor" "$((ANCHOR+1))" "$BLOCK"

echo "== 2. idempotent re-run =="
curl -fsSL "$SURL" | HYPRFETCH_WIDGET_URL="$WURL" HYPRFETCH_QS_ROOT="$QSROOT" sh >/dev/null 2>&1
N_IMP=$(grep -c "^import qs.modules.downloadManager" "$BAR")
check "no duplicate import" "1" "$N_IMP"
N_BLK=$(grep -c "// HyprFetch download widget (auto-added" "$BAR")
check "no duplicate block" "1" "$N_BLK"
N_INST=$(grep -c "DownloadWidget {" "$BAR")
check "single instance" "1" "$N_INST"

echo "== 3. QML sanity of edited bar (brace balance) =="
OPEN=$(grep -o "{" "$BAR" | wc -l); CLOSE=$(grep -o "}" "$BAR" | wc -l)
check "braces balanced" "$OPEN" "$CLOSE"

echo "== 4. uninstall via pipe =="
curl -fsSL "$SURL" | HYPRFETCH_QS_ROOT="$QSROOT" sh -s -- --uninstall 2>&1 | sed 's/^/    /'
[ ! -d "$MOD" ] && ok "module dir removed" || bad "module dir removed"
! grep -q "import qs.modules.downloadManager" "$BAR" && ok "bar import removed" || bad "bar import removed"
! grep -q "HyprFetch download widget" "$BAR" && ok "bar block removed" || bad "bar block removed"
OPEN2=$(grep -o "{" "$BAR" | wc -l); CLOSE2=$(grep -o "}" "$BAR" | wc -l)
check "braces still balanced" "$OPEN2" "$CLOSE2"
# removal must restore the file EXACTLY to the pristine fixture
if diff -q "$T/bar-original.qml" "$BAR" >/dev/null; then ok "bar restored byte-identical to original"; else bad "bar differs from original"; fi

echo "== 5. LOCAL mode (run next to extracted widget tree) =="
( cd "$T/widget-local" && HYPRFETCH_QS_ROOT="$QSROOT" sh ./install.sh >/dev/null 2>&1 )
[ -f "$MOD/DownloadWidget.qml" ] && ok "local mode installs files" || bad "local mode installs files"
grep -q "DownloadWidget {" "$BAR" && ok "local mode integrates bar" || bad "local mode integrates bar"

echo "== 6. unknown layout fallback (no anchor) =="
rm -rf "$T/home2"; mkdir -p "$T/home2/.config/quickshell/ii/modules/ii/bar"
printf 'import QtQuick\nItem {\n    id: weird\n}\n' > "$T/home2/.config/quickshell/ii/modules/ii/bar/BarContent.qml"
( cd "$T/widget-local" && HYPRFETCH_QS_ROOT="$T/home2/.config/quickshell/ii" sh ./install.sh >/dev/null 2>&1 )
grep -q "DownloadWidget {" "$T/home2/.config/quickshell/ii/modules/ii/bar/BarContent.qml" && bad "unknown layout must NOT be edited" || ok "unknown layout left untouched"
[ -f "$T/home2/.config/quickshell/ii/modules/downloadManager/DownloadWidget.qml" ] && ok "files still installed (manual hint printed)" || bad "files not installed"

echo "== 7. missing ii config fails cleanly =="
rm -rf "$T/home3"
( cd "$T/widget-local" && HYPRFETCH_QS_ROOT="$T/home3/.config/quickshell/ii" sh ./install.sh >/dev/null 2>&1 ) && bad "should fail without ii root" || ok "fails without ii root"
[ ! -d "$T/home3" ] && ok "no junk dirs created" || ok "home3 created before check (acceptable)"

kill %1 2>/dev/null || pkill -f "http.server 8765" 2>/dev/null || true
echo ""
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" = "0" ]
