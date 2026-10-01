#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# e2e_v064.sh — v0.6.4 "media engine: full speed, full ladder" end-to-end.
#
# What is really exercised (no mocks for the media path):
#   1. extension JS syntax gates (background / content / popup)
#   2. extension packages build + carry content.js @0.6.4
#   3. REAL daemon on a scratch HOME: /api/server reports 0.6.4
#   4. /api/media/ytdlp reports the new `deno` (JS runtime) status field
#   5. PROBE CACHE: two identical probes — the second must come from the
#      cache (< 500 ms vs seconds) with the identical ladder
#   6. SINGLEFLIGHT: 3 concurrent probes for the same URL all succeed
#   7. DOUBLE-CLICK GUARD: two rapid /api/media/download calls for the same
#      video+quality → the second is refused with a friendly 409 (the
#      v0.6.3 "Unable to rename file .part-Frag72.part" repro)
#   8. quality-picked download → task completes → real file on disk with a
#      non-null total_bytes
#
# Usage: scripts/e2e_v064.sh [path-to-hyprfetch-binary]
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
check() {
  if [ "$2" -eq 0 ]; then
    echo "  ✓ $1"; PASS=$((PASS + 1))
  else
    echo "  ✗ $1"; FAIL=$((FAIL + 1))
  fi
}

WORK="$(mktemp -d)"
export WORK
SERVE_PID=""
cleanup() {
  [ -n "$SERVE_PID" ] && kill "$SERVE_PID" 2>/dev/null
  pkill -f "hyprfetch.*7792" 2>/dev/null
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

echo "== 2. extension packages (v0.6.4) =="
bash scripts/build_extension.sh >"$WORK/pkg.log" 2>&1
check "build_extension.sh" "$?"
python3 - <<'PY'
import json, sys, zipfile
for path, bg in [
    ("build/extension/hyprfetch-extension-0.6.4-chrome.zip", "service_worker"),
    ("build/extension/hyprfetch-extension-0.6.4-firefox.xpi", "scripts"),
]:
    with zipfile.ZipFile(path) as z:
        m = json.loads(z.read("manifest.json"))
        assert m["version"] == "0.6.4", path
        assert "content.js" in z.namelist(), path
        bg_js = m["background"].get(bg) if isinstance(m["background"], dict) else m["background"]
        assert bg_js, path
print("packages OK")
PY
check "packages valid @0.6.4" "$?"

echo "== 3. real daemon (scratch HOME, bind 127.0.0.1:7792) =="
export HOME="$WORK/home"
mkdir -p "$HOME"
HYPRFETCH_BIND=127.0.0.1:7792 "$BIN" serve &>"$WORK/serve.log" &
SERVE_PID=$!
API=""
for _ in $(seq 1 60); do
  if curl -sf -o "$WORK/status.json" http://127.0.0.1:7792/api/server 2>/dev/null; then
    API="http://127.0.0.1:7792"
    break
  fi
  sleep 0.5
done
[ -n "$API" ] && check "daemon up on $API" 0 || { check "daemon up on $API" 1; echo "serve.log:"; tail -20 "$WORK/serve.log"; exit 1; }
python3 -c "import json,sys; s=json.load(open('$WORK/status.json')); sys.exit(0 if s.get('version')=='0.6.4' else 1)"
check "/api/server reports version 0.6.4" "$?"

echo "== 4. media engine status (yt-dlp + JS runtime field) =="
curl -sf "$API/api/media/ytdlp" -o "$WORK/ytdlp.json"
python3 - <<'PY'
import json, os, sys
d = json.load(open(os.path.join(os.environ["WORK"], "ytdlp.json")))
assert "deno" in d, f"missing deno field: {d}"
assert isinstance(d["deno"], bool)
assert "ffmpeg" in d and "installed" in d
PY
check "GET /api/media/ytdlp carries deno status" "$?"

echo "== 5. probe cache (the extension-speed fix) =="
URL="https://soundcloud.com/forss/flickermood"
T0=$(date +%s%N)
code=$(curl -s -o "$WORK/probe1.json" -w "%{http_code}" -X POST "$API/api/media/probe" \
  -H 'Content-Type: application/json' -d "{\"url\":\"$URL\"}" --max-time 120)
T1=$(date +%s%N)
FIRST_MS=$(( (T1 - T0) / 1000000 ))
check "first probe answered ($code, ${FIRST_MS}ms)" "$([ "$code" = "200" ] && echo 0 || echo 1)"
if [ "$code" != "200" ]; then
  echo "  (network/bot-wall — dumping error and skipping cache assertions)"
  cat "$WORK/probe1.json" 2>/dev/null | head -3
else
  T2=$(date +%s%N)
  code2=$(curl -s -o "$WORK/probe2.json" -w "%{http_code}" -X POST "$API/api/media/probe" \
    -H 'Content-Type: application/json' -d "{\"url\":\"$URL\"}" --max-time 30)
  T3=$(date +%s%N)
  SECOND_MS=$(( (T3 - T2) / 1000000 ))
  check "second probe answered ($code2, ${SECOND_MS}ms — cache)" "$([ "$code2" = "200" ] && echo 0 || echo 1)"
  check "cached probe is fast (< 500 ms; was ${SECOND_MS}ms)" "$([ "$SECOND_MS" -lt 500 ] && echo 0 || echo 1)"
  python3 - <<'PY'
import json, os, sys
a = json.load(open(os.path.join(os.environ.get("WORK", ""), "probe1.json")))
b = json.load(open(os.path.join(os.environ.get("WORK", ""), "probe2.json")))
qa = (a.get("media") or {}).get("qualities") or []
qb = (b.get("media") or {}).get("qualities") or []
sys.exit(0 if [q["id"] for q in qa] == [q["id"] for q in qb] and qa else 1)
PY
  check "cached ladder identical (same ids, non-empty)" "$?"

  HAS_SIZES=$(python3 - <<'PY'
import json, os
d = json.load(open(os.path.join(os.environ.get("WORK", ""), "probe1.json")))
qs = (d.get("media") or {}).get("qualities") or []
with_size = [q for q in qs if q.get("size_bytes")]
est = [q for q in qs if q.get("size_est")]
print(f"{len(with_size)}/{len(qs)} sized, {len(est)} estimated")
PY
)
  echo "  (sizes: $HAS_SIZES)"
fi

echo "== 6. singleflight (3 concurrent probes, one extraction) =="
if [ "$code" = "200" ]; then
  rm -f "$WORK/pa.json" "$WORK/pb.json" "$WORK/pc.json"
  curl -s -o "$WORK/pa.json" -X POST "$API/api/media/probe" -H 'Content-Type: application/json' -d "{\"url\":\"$URL\"}" --max-time 30 &
  P1=$!
  curl -s -o "$WORK/pb.json" -X POST "$API/api/media/probe" -H 'Content-Type: application/json' -d "{\"url\":\"$URL\"}" --max-time 30 &
  P2=$!
  curl -s -o "$WORK/pc.json" -X POST "$API/api/media/probe" -H 'Content-Type: application/json' -d "{\"url\":\"$URL\"}" --max-time 30 &
  P3=$!
  wait "$P1" "$P2" "$P3"
  python3 - <<'PY'
import json, os, sys
W = os.environ.get("WORK", "")
docs = [json.load(open(os.path.join(W, f))) for f in ("pa.json", "pb.json", "pc.json")]
ids = [tuple(q["id"] for q in ((d.get("media") or {}).get("qualities") or [])) for d in docs]
sys.exit(0 if all(x == ids[0] and ids[0] for x in ids) else 1)
PY
  check "all concurrent probes return the same ladder" "$?"
else
  echo "  (skip — first probe failed)"
fi

echo "== 7. double-click guard (the rename-error repro) =="
if [ "$code" = "200" ]; then
  QID=$(python3 - <<'PY'
import json, os
d = json.load(open(os.path.join(os.environ.get("WORK", ""), "probe1.json")))
qs = (d.get("media") or {}).get("qualities") or []
pick = next((q for q in qs if not q.get("audio_only")), qs[0] if qs else None)
print(pick["id"] if pick else "")
PY
)
  # First click → task created.
  c1=$(curl -s -o "$WORK/dl1.json" -w "%{http_code}" -X POST "$API/api/media/download" \
    -H 'Content-Type: application/json' -d "{\"url\":\"$URL\",\"quality\":\"$QID\"}" --max-time 60)
  check "first download accepted ($c1)" "$([ "$c1" = "201" ] && echo 0 || echo 1)"
  TASK_ID=$(python3 -c "import json; print(json.load(open('$WORK/dl1.json')).get('id',''))" 2>/dev/null)
  SAVE_PATH=$(python3 -c "import json; print(json.load(open('$WORK/dl1.json')).get('save_path',''))" 2>/dev/null)
  echo "  (task $TASK_ID → $SAVE_PATH)"
  # Second click immediately after → must be REFUSED, not double-spawned.
  c2=$(curl -s -o "$WORK/dl2.json" -w "%{http_code}" -X POST "$API/api/media/download" \
    -H 'Content-Type: application/json' -d "{\"url\":\"$URL\",\"quality\":\"$QID\"}" --max-time 60)
  check "second download refused with HTTP 409 (got $c2)" "$([ "$c2" = "409" ] && echo 0 || echo 1)"
  python3 - <<'PY'
import json, os, sys
d = json.load(open(os.path.join(os.environ.get("WORK", ""), "dl2.json")))
msg = json.dumps(d).lower()
sys.exit(0 if "already" in msg else 1)
PY
  check "refusal explains itself ('already queued or running')" "$?"
else
  echo "  (skip — probe failed)"
fi

echo "== 8. quality-picked download completes (real file) =="
if [ -n "${TASK_ID:-}" ] && [ "${c1:-}" = "201" ]; then
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
  python3 - <<'PY'
import json, os, sys
d = json.load(open(os.path.join(os.environ.get("WORK", ""), "task.json")))
d = d.get("task") or d
sys.exit(0 if d.get("total_bytes") else 1)
PY
  check "total_bytes is set (no more '?')" "$?"
  [ -f "$SAVE_PATH" ] && check "file on disk: $SAVE_PATH ($(stat -c%s "$SAVE_PATH" 2>/dev/null) bytes)" 0 \
    || check "file on disk: $SAVE_PATH" 1
else
  echo "  (skip — no task)"
fi

echo
echo "================================"
echo "E2E v0.6.4: $PASS passed, $FAIL failed"
echo "================================"
[ "$FAIL" -eq 0 ]
