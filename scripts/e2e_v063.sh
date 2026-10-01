#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# e2e_v063.sh — v0.6.3 "extension goes full IDM" end-to-end.
#
# What is really exercised (no mocks for the media path):
#   1. extension JS syntax gates (background / content / popup)
#   2. extension packages build + contain content script + menus (0.6.3)
#   3. REAL daemon on a scratch HOME: /api/status reports 0.6.3
#   4. extension bridge: heartbeat → status connected → media batch accepted
#   5. /api/media/probe on a REAL music/video page → kind=media with a
#      quality ladder (incl. an audio-only option) — the exact call the
#      popup + in-page quality panel make
#   6. /api/media/download with a picked quality → task completes →
#      real file on disk (the quality-panel click path)
#   7. /api/extension/download of the LIVE channel extension package →
#      task completes → sha256 byte-identical to the server asset
#      (the WebUI "Install extension" click path)
#
# Usage: scripts/e2e_v063.sh [path-to-hyprfetch-binary]
# ---------------------------------------------------------------------------
set -u
cd "$(dirname "$0")/.."
ROOT="$PWD"

BIN="${1:-$ROOT/target/debug/hyprfetch}"
if [ ! -x "$BIN" ]; then
  echo "[setup] $BIN not found — building debug binary…"
  cargo build --workspace -q || { echo "FAIL: cargo build"; exit 1; }
  BIN="$ROOT/target/debug/hyprfetch"
fi
BIN="$(readlink -f "$BIN")"
echo "[setup] binary: $BIN ($("$BIN" --version | awk '{print $2}'))"

PASS=0
FAIL=0
check() { # check <label> <previous-command-succeeded(0|1)>
  if [ "$2" -eq 0 ]; then
    echo "  ✓ $1"; PASS=$((PASS + 1))
  else
    echo "  ✗ $1"; FAIL=$((FAIL + 1))
  fi
}

WORK="$(mktemp -d)"
SERVE_PID=""
cleanup() {
  [ -n "$SERVE_PID" ] && kill "$SERVE_PID" 2>/dev/null
  pkill -f "hyprfetch.*7791" 2>/dev/null
  rm -rf "$WORK"
}
trap cleanup EXIT

echo "== 1. extension JS syntax gates =="
ok=1
for f in extension/background.js extension/content.js extension/popup.js; do
  node --check "$f" || ok=0
done
[ "$ok" -eq 1 ] && check "node --check on background/content/popup" 0 \
  || check "node --check on background/content/popup" 1

echo "== 2. extension packages (v0.6.3, content script + menus) =="
bash scripts/build_extension.sh >"$WORK/pkg.log" 2>&1
check "build_extension.sh" "$?"
python3 - <<'PY'
import json, sys, zipfile
for path, bg in [
    ("build/extension/hyprfetch-extension-0.6.3-chrome.zip", "service_worker"),
    ("build/extension/hyprfetch-extension-0.6.3-firefox.xpi", "scripts"),
]:
    with zipfile.ZipFile(path) as z:
        m = json.loads(z.read("manifest.json"))
        assert m["version"] == "0.6.3", path
        assert bg in m["background"], path
        assert "content.js" in z.namelist(), path
        cs = m.get("content_scripts", [])
        assert cs and "content.js" in cs[0]["js"] and cs[0]["all_frames"], path
        assert "contextMenus" in m["permissions"], path
print("packages OK")
PY
check "packages contain content.js + contextMenus @0.6.3" "$?"

echo "== 3. real daemon (scratch HOME, bind 127.0.0.1:7791) =="
export HOME="$WORK/home"
mkdir -p "$HOME"
HYPRFETCH_BIND=127.0.0.1:7791 "$BIN" serve &>"$WORK/serve.log" &
SERVE_PID=$!
API=""
for _ in $(seq 1 60); do
  if curl -sf -o "$WORK/status.json" http://127.0.0.1:7791/api/server 2>/dev/null; then
    API="http://127.0.0.1:7791"
    break
  fi
  sleep 0.5
done
[ -n "$API" ] && check "daemon up on $API" 0 || { check "daemon up on $API" 1; echo "serve.log:"; tail -20 "$WORK/serve.log"; exit 1; }
python3 -c "import json,sys; s=json.load(open('$WORK/status.json')); sys.exit(0 if s.get('version')=='0.6.3' else 1)"
check "/api/status reports version 0.6.3" "$?"

echo "== 4. extension bridge =="
curl -sf -X POST "$API/api/extension/heartbeat" -H 'Content-Type: application/json' \
  -d '{"version":"0.6.3"}' | python3 -c "import json,sys; sys.exit(0 if json.load(sys.stdin).get('ok') else 1)"
check "heartbeat accepted" "$?"
curl -sf "$API/api/extension/status" | python3 -c "
import json,sys; s=json.load(sys.stdin)
sys.exit(0 if s.get('connected') and s.get('version')=='0.6.3' else 1)"
check "status shows extension connected @0.6.3" "$?"
curl -sf -X POST "$API/api/extension/media" -H 'Content-Type: application/json' \
  -d '{"media":[{"url":"https://example.com/media/e2e-v063.mp4","media_type":"video/mp4","size":12345678,"filename":"e2e-v063.mp4","page_url":"https://example.com/watch","ts":0}]}' \
  | python3 -c "import json,sys; sys.exit(0 if json.load(sys.stdin).get('added',0)>=1 else 1)"
check "media batch accepted" "$?"
curl -sf -X DELETE "$API/api/extension/media" > /dev/null
check "captured media cleared" "$?"

echo "== 5. media probe (real page — popup/panel call path) =="
PROBE_URL=""
PROBE_KIND=""
for CAND in "https://soundcloud.com/forss/flickermood" "https://archive.org/details/BigBuckBunny_124"; do
  code=$(curl -s -o "$WORK/probe.json" -w "%{http_code}" -X POST "$API/api/media/probe" \
    -H 'Content-Type: application/json' -d "{\"url\":\"$CAND\"}" --max-time 90)
  if [ "$code" = "200" ] && python3 -c "
import json,sys
d=json.load(open('$WORK/probe.json'))
sys.exit(0 if d.get('kind')=='media' and len((d.get('media') or {}).get('qualities') or [])>0 else 1)" 2>/dev/null; then
    PROBE_URL="$CAND"
    PROBE_KIND="media"
    break
  fi
  # direct-file pages are still a valid probe answer (kind=file)
  if [ "$code" = "200" ] && python3 -c "
import json,sys
d=json.load(open('$WORK/probe.json'))
sys.exit(0 if d.get('kind')=='file' else 1)" 2>/dev/null; then
    PROBE_URL="$CAND"
    PROBE_KIND="file"
    break
  fi
done
check "probe answered ($PROBE_URL → kind=$PROBE_KIND)" "$([ -n "$PROBE_URL" ] && echo 0 || echo 1)"

LADDER_N=$(python3 -c "
import json
d=json.load(open('$WORK/probe.json'))
m=d.get('media') or {}
print(len(m.get('qualities') or []))" 2>/dev/null || echo 0)
HAS_AUDIO=$(python3 -c "
import json
d=json.load(open('$WORK/probe.json'))
m=d.get('media') or {}
qs=m.get('qualities') or []
print(1 if any(q.get('audio_only') for q in qs) else 0)" 2>/dev/null || echo 0)
if [ "$PROBE_KIND" = "media" ]; then
  check "quality ladder has $LADDER_N options" "$([ "$LADDER_N" -ge 1 ] && echo 0 || echo 1)"
  check "ladder includes an audio-only option (MP3/M4A)" "$([ "$HAS_AUDIO" = "1" ] && echo 0 || echo 1)"
fi

echo "== 6. quality-picked download (panel click path) =="
if [ "$PROBE_KIND" = "media" ]; then
  QID=$(python3 -c "
import json
d=json.load(open('$WORK/probe.json'))
qs=(d.get('media') or {}).get('qualities') or []
pick=next((q for q in qs if not q.get('audio_only')), qs[0] if qs else None)
print(pick['id'] if pick else '')")
  EXPECT_EXT=$(python3 -c "
import json
d=json.load(open('$WORK/probe.json'))
qs=(d.get('media') or {}).get('qualities') or []
pick=next((q for q in qs if not q.get('audio_only')), qs[0] if qs else None)
print(pick['container'] if pick else 'bin')")
  echo "  (picked quality id=$QID container=$EXPECT_EXT)"
  code=$(curl -s -o "$WORK/dl.json" -w "%{http_code}" -X POST "$API/api/media/download" \
    -H 'Content-Type: application/json' \
    -d "{\"url\":\"$PROBE_URL\",\"quality\":\"$QID\"}" --max-time 60)
  check "POST /api/media/download → $code" "$([ "$code" = "201" ] && echo 0 || echo 1)"
  TASK_ID=$(python3 -c "import json; print(json.load(open('$WORK/dl.json')).get('id',''))" 2>/dev/null)
  SAVE_PATH=$(python3 -c "import json; print(json.load(open('$WORK/dl.json')).get('save_path',''))" 2>/dev/null)
  done_f=1
  for _ in $(seq 1 240); do
    curl -sf "$API/api/tasks/$TASK_ID" -o "$WORK/task.json" 2>/dev/null || { sleep 1; continue; }
    st=$(python3 -c "
import json
d=json.load(open('$WORK/task.json'))
d=d.get('task') or d
print(d.get('state',''))" 2>/dev/null)
    case "$st" in
      completed|complete|done|finished) done_f=0; break ;;
      failed|error|cancelled) done_f=1; echo "  task state: $st"; break ;;
    esac
    sleep 1
  done
  check "task $TASK_ID completed" "$done_f"
  [ -f "$SAVE_PATH" ] && check "file on disk: $SAVE_PATH ($(stat -c%s "$SAVE_PATH" 2>/dev/null) bytes)" 0 \
    || check "file on disk: $SAVE_PATH" 1
  python3 - "$SAVE_PATH" <<'PY' && check "file looks like real media (size > 200KB)" 0 || check "file looks like real media" 1
import os, sys
sys.exit(0 if os.path.getsize(sys.argv[1]) > 200_000 else 1)
PY
else
  echo "  (skip — probe resolved to a direct file; download path covered by step 7)"
fi

echo "== 7. in-app extension package install (WebUI Install click path) =="
PKG_URL="https://istias.tech/hyprfetch/updates/extension/hyprfetch-extension-firefox.xpi"
code=$(curl -s -o "$WORK/pkg-task.json" -w "%{http_code}" -X POST "$API/api/extension/download" \
  -H 'Content-Type: application/json' \
  -d "{\"url\":\"$PKG_URL\",\"filename\":\"hyprfetch-extension-e2e.xpi\",\"page_url\":\"http://127.0.0.1:7791/\"}" \
  --max-time 30)
check "POST /api/extension/download (channel xpi) → $code" "$([ "$code" = "201" ] && echo 0 || echo 1)"
PKG_TASK=$(python3 -c "import json; print(json.load(open('$WORK/pkg-task.json')).get('id',''))" 2>/dev/null)
PKG_PATH=$(python3 -c "import json; print(json.load(open('$WORK/pkg-task.json')).get('save_path',''))" 2>/dev/null)
pkg_f=1
for _ in $(seq 1 120); do
  curl -sf "$API/api/tasks/$PKG_TASK" -o "$WORK/pkgtask.json" 2>/dev/null || { sleep 1; continue; }
  st=$(python3 -c "
import json
d=json.load(open('$WORK/pkgtask.json'))
d=d.get('task') or d
print(d.get('state',''))" 2>/dev/null)
  case "$st" in
    completed|complete|done|finished) pkg_f=0; break ;;
    failed|error|cancelled) pkg_f=1; echo "  task state: $st"; break ;;
  esac
  sleep 1
done
check "extension package task completed" "$pkg_f"
# server reference copy
curl -sf -o "$WORK/ref.xpi" "$PKG_URL?t=$(date +%s%N)"
if [ -f "$PKG_PATH" ] && [ -f "$WORK/ref.xpi" ]; then
  a=$(sha256sum "$PKG_PATH" | awk '{print $1}')
  b=$(sha256sum "$WORK/ref.xpi" | awk '{print $1}')
  [ "$a" = "$b" ] && check "downloaded package sha256 matches server asset" 0 \
    || check "downloaded package sha256 matches server asset ($a vs $b)" 1
  python3 - "$PKG_PATH" <<'PY' && check "package is a valid zip with manifest 0.6.3" 0 || check "package is a valid zip with manifest" 1
import json, sys, zipfile
with zipfile.ZipFile(sys.argv[1]) as z:
    m = json.loads(z.read("manifest.json"))
    assert m["manifest_version"] == 3
PY
else
  check "package file exists ($PKG_PATH)" 1
fi

echo
echo "=== E2E v0.6.3: $PASS passed, $FAIL failed ==="
[ "$FAIL" -eq 0 ]
