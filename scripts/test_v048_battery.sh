#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# test_v048_battery.sh — v0.4.8 Quickshell-widget backend battery.
#
# Covers everything the bar widget depends on:
#   A. status file: the daemon writes
#      $XDG_DATA_HOME/download-manager/status.json with the EXACT widget
#      schema (active_downloads / recent_downloads / last_completed),
#      updates it live during downloads (progress/speed/eta), and empties
#      active on completion (recent + last_completed filled).
#   B. `hyprfetch add <url>` — works when the daemon is ALREADY running and
#      AUTO-STARTS the daemon when it isn't; rejects bad URLs; supports
#      multiple URLs in one call.
#   C. `hyprfetch reveal <id>` — 404 path fails cleanly, success path
#      answers via the daemon.
#   D. event-driven writes: at idle the file does NOT churn (throttled).
#
# Usage: scripts/test_v048_battery.sh [path-to-hyprfetch-binary]
# ---------------------------------------------------------------------------
set -u
cd "$(dirname "$0")/.."
ROOT="$PWD"
BIN="${1:-$ROOT/target/release/hyprfetch}"
PORT=$(( 20000 + RANDOM % 20000 ))
PASS=0; FAIL=0

check() { # check <label> <exit-code>
  if [ "$2" = "0" ]; then echo "  ✓ $1"; PASS=$((PASS+1)); else echo "  ✗ $1"; FAIL=$((FAIL+1)); fi
}

cleanup() {
  [ -n "${SRV_PID:-}" ] && kill "$SRV_PID" 2>/dev/null
  [ -n "${MOCK_PID:-}" ] && kill "$MOCK_PID" 2>/dev/null
}
trap cleanup EXIT

echo "=== v0.4.8 battery: binary $BIN ==="
"$BIN" --version >/dev/null 2>&1
check "binary runs" $?

# Isolated environment: temp DB + downloads + XDG data (the widget status
# file lives under XDG_DATA_HOME). The daemon auto-started by `add` inherits
# these. HYPRFETCH_ALLOW_PRIVATE lets the engine download from the local
# mock (SSRF protection would otherwise block 127.0.0.1).
TMP=$(mktemp -d)
export XDG_DATA_HOME="$TMP/data"
export HYPRFETCH_DB="$TMP/test.db"
export HYPRFETCH_DOWNLOAD_DIR="$TMP/downloads"
export HYPRFETCH_ALLOW_PRIVATE=true
# The folder/file opener runs in the DAEMON (reveal comes from the widget),
# so the stub must be in the daemon's environment too.
export HYPRFETCH_FILE_OPENER=/bin/true
STATUS="$XDG_DATA_HOME/download-manager/status.json"
mkdir -p "$XDG_DATA_HOME" "$TMP/served"
echo "status dir: $STATUS"

# Kill any pre-existing daemon so the battery owns the lifecycle.
"$BIN" daemon stop >/dev/null 2>&1
sleep 1

# --- mock download payload: ~6 MB served SLOWLY (~1.2 MB/s) so progress,
# speed and eta are observable in the status file before completion --------
head -c 6000000 /dev/urandom > "$TMP/served/blob.bin"
python3 - "$TMP/served" "$PORT" <<'PYEOF' &
import http.server, os, sys, socketserver, time
os.chdir(sys.argv[1])
class SlowHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *a): pass
    def copyfile(self, src, dst):
        # 32 KB per 25 ms ~= 1.25 MB/s — slow enough to watch.
        while True:
            chunk = src.read(32768)
            if not chunk:
                break
            dst.write(chunk)
            time.sleep(0.025)
with socketserver.ThreadingTCPServer(("127.0.0.1", int(sys.argv[2])), SlowHandler) as s:
    s.serve_forever()
PYEOF
MOCK_PID=$!
for i in $(seq 1 50); do curl -s -o /dev/null "http://127.0.0.1:$PORT/" && break; sleep 0.1; done
URL="http://127.0.0.1:$PORT/blob.bin"

# --- scenario A1: add with daemon DOWN auto-starts the daemon ---------------
"$BIN" add "$URL" > "$TMP/add1.out" 2>&1
check "hyprfetch add (daemon down) exits 0" $?
grep -q "blob.bin" "$TMP/add1.out"
check "add prints the created filename" $?
grep -q "starting it in the background" "$TMP/add1.out"
check "add auto-started the daemon" $?

# --- scenario A2: status file exists with the exact widget schema -----------
ok=0
for i in $(seq 1 60); do
  [ -f "$STATUS" ] && break
  sleep 0.25
done
[ -f "$STATUS" ]
check "status.json exists after start" $?

python3 - "$STATUS" <<'PYEOF'
import json, re, sys
d = json.load(open(sys.argv[1]))
assert set(d.keys()) == {"active_downloads", "recent_downloads", "last_completed"}, d.keys()
for a in d["active_downloads"]:
    assert {"id","filename","progress","speed","eta"} <= set(a.keys()), a
    assert isinstance(a["progress"], (int, float)) and 0.0 <= a["progress"] <= 100.0
    assert re.match(r"^\d+(\.\d+)? (B|KB|MB|GB)/s$", a["speed"]), a["speed"]
    assert re.match(r"^(\d{2}:\d{2}:\d{2}|-{2}:-{2}:-{2})$", a["eta"]), a["eta"]
PYEOF
check "schema keys + active entry shape (python strict parse)" $?

# --- scenario A3: progress becomes observable and moves ---------------------
prev=-1
moved=0
for i in $(seq 1 80); do
  cur=$(python3 -c "
import json,sys
try: d=json.load(open('$STATUS'))
except Exception: print(-1); raise SystemExit
a=d['active_downloads']
print(round(a[0]['progress'],1) if a else -1)
" 2>/dev/null)
  if [ "$prev" != "-1" ] && [ "$cur" != "-1" ] && [ "$cur" != "$prev" ]; then moved=1; fi
  prev=$cur
  [ "$moved" = "1" ] && [ -n "$(python3 -c "
import json
d=json.load(open('$STATUS'))
a=d['active_downloads']
print('x' if a and a[0]['progress'] > 2 else '')
" 2>/dev/null)" ] && break
  sleep 0.25
done
[ "$moved" = "1" ]
check "progress updates are observable in the file (event-driven writes)" $?

speed=$(python3 -c "
import json
d=json.load(open('$STATUS'))
a=d['active_downloads']
print(a[0]['speed'] if a else '')
")
echo "$speed" | grep -qE "^[0-9.]+ (B/|KB/|MB/|GB/)s$"
check "speed is human formatted ($speed)" $?

# --- scenario A4: completion empties active, fills recent + last_completed --
task_id=$(python3 -c "
import json
d=json.load(open('$STATUS'))
print(d['active_downloads'][0]['id'] if d['active_downloads'] else '')
")
ok=0
for i in $(seq 1 120); do
  n_active=$(python3 -c "
import json
d=json.load(open('$STATUS'))
print(len(d['active_downloads']))
" 2>/dev/null)
  n_recent=$(python3 -c "
import json
d=json.load(open('$STATUS'))
print(len(d['recent_downloads']))
" 2>/dev/null)
  if [ "$n_active" = "0" ] && [ "${n_recent:-0}" -ge 1 ]; then ok=1; break; fi
  sleep 0.5
done
[ "$ok" = "1" ]
check "on completion: active empties, recent fills" $?

python3 - "$STATUS" <<'PYEOF'
import json, sys
d = json.load(open(sys.argv[1]))
assert d["active_downloads"] == [], d["active_downloads"]
r = d["recent_downloads"][0]
assert {"id","filename","status","timestamp"} <= set(r.keys()), r
assert r["status"] == "completed", r
assert r["filename"] == "blob.bin", r
assert r["timestamp"] > 1_500_000_000, r  # unix SECONDS
lc = d["last_completed"]
assert lc["filename"] == "blob.bin" and lc["timestamp"] == r["timestamp"], lc
PYEOF
check "recent entry + last_completed exact (completed, seconds ts)" $?

# --- scenario B1: add with daemon RUNNING (second call, no re-start) --------
"$BIN" add "$URL" > "$TMP/add2.out" 2>&1
check "hyprfetch add (daemon up) exits 0" $?
if grep -q "starting it in the background" "$TMP/add2.out"; then
  check "second add does not restart the daemon" 1
else
  check "second add does not restart the daemon" 0
fi

# wait for completion of both, then check recent has 2 entries
ok=0
for i in $(seq 1 120); do
  n=$(python3 -c "
import json
d=json.load(open('$STATUS'))
print(len([r for r in d['recent_downloads'] if r['status']=='completed']))
" 2>/dev/null)
  [ "${n:-0}" -ge 2 ] && ok=1 && break
  sleep 0.5
done
[ "$ok" = "1" ]
check "both downloads land in recent_downloads (capped list works)" $?

# --- scenario B2: multi-URL add in one call ---------------------------------
"$BIN" add "$URL" "$URL" > "$TMP/add3.out" 2>&1
check "multi-url add exits 0" $?
[ "$(grep -c '→' "$TMP/add3.out")" = "2" ]
check "multi-url add prints both tasks" $?

# --- scenario B3: bad URL rejected client-side ------------------------------
"$BIN" add "ftp://nope/x.bin" > "$TMP/add4.out" 2>&1
[ $? != 0 ]
check "ftp:// URL rejected" $?
grep -q "invalid URL" "$TMP/add4.out"
check "rejection explains http(s) requirement" $?

# --- scenario C: reveal ------------------------------------------------------
"$BIN" reveal "t_does_not_exist" > "$TMP/rev1.out" 2>&1
[ $? != 0 ]
check "reveal of unknown id fails cleanly" $?
grep -q "no such download" "$TMP/rev1.out"
check "reveal 404 message is clear" $?

ok=0
for i in $(seq 1 120); do
  n=$(python3 -c "
import json
d=json.load(open('$STATUS'))
print(len([r for r in d['recent_downloads'] if r['status']=='completed']))
" 2>/dev/null)
  [ "${n:-0}" -ge 4 ] && ok=1 && break
  sleep 0.5
done
rid=$(python3 -c "
import json
d=json.load(open('$STATUS'))
done=[r for r in d['recent_downloads'] if r['status']=='completed']
print(done[0]['id'] if done else '')
")
if [ -n "$rid" ]; then
  HYPRFETCH_FILE_OPENER=/bin/true "$BIN" reveal "$rid" > "$TMP/rev2.out" 2>&1
  check "reveal of a completed task answers 200 (opener stubbed)" $?
else
  check "reveal of a completed task answers 200 (opener stubbed)" 1
fi

# --- scenario D: idle file does not churn (event-driven, throttled) ---------
before=$(stat -c %Y "$STATUS" 2>/dev/null)
sleep 4
after=$(stat -c %Y "$STATUS" 2>/dev/null)
[ "$before" = "$after" ]
check "no writes while idle (file mtime unchanged over 4s)" $?

# --- scenario E: fresh daemon start rebuilds state from the DB --------------
"$BIN" daemon stop >/dev/null 2>&1
sleep 1
"$BIN" daemon start >/dev/null 2>&1
sleep 2
python3 - "$STATUS" <<'PYEOF'
import json, sys
d = json.load(open(sys.argv[1]))
assert set(d.keys()) == {"active_downloads", "recent_downloads", "last_completed"}
assert len(d["recent_downloads"]) >= 4, d["recent_downloads"]
lc = d["last_completed"]
assert lc is not None and lc["filename"] == "blob.bin"
PYEOF
check "daemon restart rebuilds recent/last_completed from the DB" $?

# --- scenario F: doctor + update --check unaffected --------------------------
"$BIN" doctor > "$TMP/doctor.out" 2>&1
check "doctor still OK" $?
HYPRFETCH_UPDATE_CHANNEL="" "$BIN" update --check > "$TMP/upd.out" 2>&1
check "update --check still OK (channel disabled)" $?

# leave a clean state for the next battery run
"$BIN" daemon stop >/dev/null 2>&1

echo
echo "=== v0.4.8 battery: $PASS passed, $FAIL failed ==="
[ "$FAIL" = "0" ]
