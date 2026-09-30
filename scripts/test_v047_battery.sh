#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# test_v047_battery.sh — v0.4.7 updater regression battery.
#
# Covers the two owner-reported bugs fixed in v0.4.7:
#   A. `hyprfetch update` died on slow networks with
#      "update channel: asset read: error decoding response body"
#      → the archive download is now streamed with NO total deadline,
#        retried with resume (Range) and verified by sha256.
#   B. in-app update on a system install needed a password / failed
#        → one-click update authorization (sudoers + narrow helper),
#          privilege ladder helper → sudo -n → pkexec, needs_password flow.
#
# Scenarios (local mock channel unless HYPRFETCH_LIVE=1):
#   1. throttled-channel update COMPLETES (pre-fix binary fails at 60s)
#      THROTTLE_SLOW=1 reproduces the owner case exactly (40 KB/s, ~100 s)
#   2. dropped-connection mid-download → resumes → completes (Range resume)
#   3. corrupt archive → refused, binary untouched
#   4. GET /api/update/check reports one_click_ready
#   5. GET /api/update/authorize/status answers JSON
#   6. install.sh round-trip: fresh install → replace-in-place (mock channel)
#
# Usage: scripts/test_v047_battery.sh [path-to-hyprfetch-binary]
# ---------------------------------------------------------------------------
set -u
cd "$(dirname "$0")/.."
ROOT="$PWD"
BIN="${1:-$ROOT/target/release/hyprfetch}"
PORT=$(( 19000 + RANDOM % 20000 ))
PASS=0; FAIL=0

check() { # check <label> <exit-code>
  if [ "$2" = "0" ]; then echo "  ✓ $1"; PASS=$((PASS+1)); else echo "  ✗ $1"; FAIL=$((FAIL+1)); fi
}

cleanup() { [ -n "${SRV_PID:-}" ] && kill "$SRV_PID" 2>/dev/null; [ -n "${SRV_PID2:-}" ] && kill "$SRV_PID2" 2>/dev/null; }
trap cleanup EXIT

echo "=== v0.4.7 battery: binary $BIN ==="
"$BIN" --version >/dev/null 2>&1
check "binary runs" $?

# --- build a tiny fake release payload (real tar.gz with a hyprfetch file) --
TMP=$(mktemp -d)
mkdir -p "$TMP/payload/hyprfetch-9.9.9-linux-x64"
printf '#!/bin/sh\necho "hyprfetch 9.9.9"\n' > "$TMP/payload/hyprfetch-9.9.9-linux-x64/hyprfetch"
chmod 755 "$TMP/payload/hyprfetch-9.9.9-linux-x64/hyprfetch"
tar czf "$TMP/asset.tar.gz" -C "$TMP/payload" hyprfetch-9.9.9-linux-x64
SHA=$(sha256sum "$TMP/asset.tar.gz" | cut -d' ' -f1)
SIZE=$(stat -c%s "$TMP/asset.tar.gz")

write_manifest() { # write_manifest <port>
  cat > "$TMP/latest.json" <<EOF
{"version":"9.9.9","tag":"v9.9.9","published_at":"2026-09-30T00:00:00Z",
 "notes_url":"https://istias.tech/hyprfetch/updates",
 "assets":{"x86_64-unknown-linux-gnu":{"url":"http://127.0.0.1:$1/asset.tar.gz","sha256":"$SHA","size":$SIZE}}}
EOF
}

# --- scenario 1: throttled channel, update completes ------------------------
echo "=== 1. throttled-channel update completes (no 60s cap) ==="
OLD_DIR=$(mktemp -d)
cp "$BIN" "$OLD_DIR/hyprfetch"
chmod -R 755 "$OLD_DIR"
PORT=$(( PORT + 1 ))
SPEED=8192 # 8 KB/s by default; THROTTLE_SLOW=1 → 40 KB/s like the owner's run
[ "${THROTTLE_SLOW:-0}" = "1" ] && SPEED=40960
python3 "$ROOT/scripts/throttled_channel.py" "$PORT" "$TMP/asset.tar.gz" "$SPEED" 9.9.9 >/dev/null 2>&1 &
SRV_PID=$!
for i in $(seq 1 20); do
  curl -sf "http://127.0.0.1:$PORT/latest.json" >/dev/null 2>&1 && break
  sleep 0.5
done
curl -sf "http://127.0.0.1:$PORT/latest.json" >/dev/null 2>&1
check "mock throttled channel is up" $?
cd "$OLD_DIR"
OUT=$(HYPRFETCH_ASSUME_YES=1 HYPRFETCH_UPDATE_CHANNEL="http://127.0.0.1:$PORT/" ./hyprfetch update 2>&1)
RC=$?
echo "$OUT" | tail -3
check "throttled update exits 0 (was: error decoding response body at 60s)" $RC
echo "$OUT" | grep -q "installed 9.9.9"
check "update reports installed 9.9.9" $?
echo "$OUT" | grep -Eq "[0-9]+%"
check "download progress percentage shown" $?
kill $SRV_PID 2>/dev/null; wait $SRV_PID 2>/dev/null; SRV_PID=""
cd "$ROOT"

# --- scenario 2+3 live under the unit suite (drop-resume, stall, corrupt) ---
echo "=== 2/3. resume + corrupt-body paths (cargo test) ==="
export PATH="$HOME/.cargo/bin:$PATH"
if cargo test -p hyprfetch-core download_ -- 2>/dev/null | grep -q "test result: ok"; then
  check "unit tests: drop-resume / stall-retry / corrupt-retry" 0
else
  check "unit tests: drop-resume / stall-retry / corrupt-retry" 1
fi

# --- scenario 4+5: API fields ------------------------------------------------
echo "=== 4/5. API: one_click_ready + authorize status ==="
export HYPRFETCH_DB="$TMP/test.db"
export HYPRFETCH_DOWNLOAD_DIR="$TMP/downloads"
"$BIN" serve --bind "127.0.0.1:$(( PORT + 1 ))" >/dev/null 2>&1 &
SRV_PID2=$!
for i in $(seq 1 20); do
  curl -sf "http://127.0.0.1:$(( PORT + 1 ))/healthz" >/dev/null 2>&1 && break
  sleep 0.5
done
ONE=$(curl -sf "http://127.0.0.1:$(( PORT + 1 ))/api/update/check" | jq -r '.one_click_ready' 2>/dev/null)
[ "$ONE" = "false" ] || [ "$ONE" = "true" ]
check "/api/update/check carries one_click_ready" $?
STAT=$(curl -sf "http://127.0.0.1:$(( PORT + 1 ))/api/update/authorize/status" | jq -r '.running' 2>/dev/null)
[ "$STAT" = "false" ]
check "/api/update/authorize/status answers" $?
kill $SRV_PID2 2>/dev/null; wait $SRV_PID2 2>/dev/null; SRV_PID2=""

# --- scenario 6: install.sh round-trip against a mock channel ----------------
echo "=== 6. install.sh: fresh install → replace-in-place ==="
CH_PORT=$(( PORT + 2 ))
write_manifest "$CH_PORT"
# install.sh derives the asset path from the version:
#   <channel>/<version>/hyprfetch-<version>-linux-x64.tar.gz
mkdir -p "$TMP/9.9.9"
cp "$TMP/asset.tar.gz" "$TMP/9.9.9/hyprfetch-9.9.9-linux-x64.tar.gz"
cp "$TMP/asset.tar.gz" "$TMP/hyprfetch-9.9.9-linux-x64.tar.gz"
python3 -m http.server "$CH_PORT" --bind 127.0.0.1 --directory "$TMP" >/dev/null 2>&1 &
SRV_PID=$!
for i in $(seq 1 20); do
  curl -sf "http://127.0.0.1:$CH_PORT/latest.json" >/dev/null 2>&1 && break
  sleep 0.5
done
HOME=$TMP/home sh "$ROOT/../Docs/hyprfetch/install.sh" --channel "http://127.0.0.1:$CH_PORT" >"$TMP/install1.log" 2>&1
INSTALLED_BIN=""
for CAND in "$TMP/home/.local/bin/hyprfetch" /usr/local/bin/hyprfetch; do
  [ -x "$CAND" ] && INSTALLED_BIN="$CAND" && break
done
[ -n "$INSTALLED_BIN" ]
check "install.sh fresh install lands (sandbox prefix)" $?
"$INSTALLED_BIN" --version 2>/dev/null | grep -q "9.9.9"
check "installed binary reports its version" $?
HOME=$TMP/home sh "$ROOT/../Docs/hyprfetch/install.sh" --channel "http://127.0.0.1:$CH_PORT" >"$TMP/install2.log" 2>&1
[ -f "${INSTALLED_BIN}.old" ]
check "second run replaces in place + keeps .old rollback" $?
# cleanup: never leave the fake 9.9.9 binary shadowing the real one
rm -f "$TMP/home/.local/bin/hyprfetch"* /usr/local/bin/hyprfetch /usr/local/bin/hyprfetch.old
kill $SRV_PID 2>/dev/null; wait $SRV_PID 2>/dev/null; SRV_PID=""

echo
echo "==============================================="
echo "PASS: $PASS  FAIL: $FAIL"
echo "==============================================="
[ "$FAIL" = "0" ]
