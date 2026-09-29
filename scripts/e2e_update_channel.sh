#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# e2e_update_channel.sh — end-to-end tests for the server-only updater.
#
# The updater talks ONLY to the self-hosted update channel (a plain HTTPS
# `latest.json` manifest + versioned archives). These scenarios run against
# a local mock channel (python3 http.server) and assert:
#   1. --check reports an available version via the channel
#   2. --check prints "up to date" when the manifest matches CARGO_PKG_VERSION
#   3. full install: download → sha256-verify → atomic swap → .old backup
#   4. tampered manifest sha256 → REFUSED, binary untouched
#   5. unreachable channel → clear error pointing at istias.tech, no GitHub
#   6. --channel "" disables the updater
#   7. web UI /api/update/check answers from the channel
#   8. malformed manifest → clean error, no crash
#
# Usage: scripts/e2e_update_channel.sh [path-to-hyprfetch-binary]
# (defaults to building the debug binary itself)
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
VERSION="$("$BIN" --version | awk '{print $2}')"
echo "[setup] binary: $BIN (version $VERSION)"

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
MOCK_PID=""
SERVE_PID=""
cleanup() { [ -n "$MOCK_PID" ] && kill "$MOCK_PID" 2>/dev/null; [ -n "$SERVE_PID" ] && kill "$SERVE_PID" 2>/dev/null; rm -rf "$WORK"; }
trap cleanup EXIT

# ---------------------------------------------------------------------------
# Mock channel: serves $WORK/channel as a web root on 127.0.0.1:8765.
# ---------------------------------------------------------------------------
python3 - "$WORK/channel" <<'PYEOF' &
import functools, http.server, pathlib, sys
root = pathlib.Path(sys.argv[1])
handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(root))
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 8765), handler)
print("mock channel ready", flush=True)
srv.serve_forever()
PYEOF
MOCK_PID=$!
for _ in $(seq 1 50); do curl -sf -o /dev/null http://127.0.0.1:8765/ && break; sleep 0.2; done
CHANNEL="http://127.0.0.1:8765"
echo "[setup] mock channel at $CHANNEL"

# Helper: build a tar.gz containing an executable script named `hyprfetch`.
make_tarball() { # make_tarball <out.tar.gz> <marker-content>
  local out="$1" marker="$2" dir
  dir="$(mktemp -d)"
  printf '#!/bin/sh\necho "%s"\n' "$marker" > "$dir/hyprfetch"
  chmod 755 "$dir/hyprfetch"
  tar -C "$dir" -czf "$out" hyprfetch
  rm -rf "$dir"
}

# Helper: rewrite the channel manifest.
write_manifest() { # write_manifest <version> [asset-url] [sha256]
  local ver="$1" url="${2:-}" sha="${3:-}"
  if [ -z "$url" ]; then
    jq -n --arg v "$ver" \
      '{version: $v, tag: ("v"+$v), published_at: "2026-09-29T00:00:00Z",
        notes_url: "https://istias.tech/hyprfetch/updates", assets: {}}' \
      > "$WORK/channel/latest.json"
  else
    jq -n --arg v "$ver" --arg u "$url" --arg s "$sha" --argjson size "$(stat -c%s "$WORK/archive.tar.gz")" \
      '{version: $v, tag: ("v"+$v), published_at: "2026-09-29T00:00:00Z",
        notes_url: "https://istias.tech/hyprfetch/updates",
        assets: {"x86_64-unknown-linux-gnu": {url: $u, sha256: $s, size: $size}}}' \
      > "$WORK/channel/latest.json"
  fi
}


echo
echo "=== 1. --check reports an available version via the channel ==="
make_tarball "$WORK/archive.tar.gz" "marker-available"
SHA=$(sha256sum "$WORK/archive.tar.gz" | cut -d' ' -f1)
mkdir -p "$WORK/channel/9.9.9"
cp "$WORK/archive.tar.gz" "$WORK/channel/9.9.9/hyprfetch-9.9.9-linux-x64.tar.gz"
write_manifest "9.9.9" "$CHANNEL/9.9.9/hyprfetch-9.9.9-linux-x64.tar.gz" "$SHA"
OUT=$("$BIN" update --check --channel "$CHANNEL" 2>&1)
echo "$OUT" | head -6
echo "$OUT" | grep -q "checked via update channel ($CHANNEL)"
check "prints 'checked via update channel'" $?
echo "$OUT" | grep -q "latest release  : 9.9.9"
check "shows latest 9.9.9" $?
echo "$OUT" | grep -q "update available: run \`hyprfetch update\`"
check "says update available" $?

echo
echo "=== 2. --check prints 'up to date' when on the newest version ==="
write_manifest "$VERSION"
OUT=$("$BIN" update --check --channel "$CHANNEL" 2>&1)
echo "$OUT"
echo "$OUT" | grep -q "→ up to date"
check "prints 'up to date'" $?

echo
echo "=== 3. full install: download → sha256-verify → atomic swap → backup ==="
SWAP_TARGET="$WORK/bin/hyprfetch"
mkdir -p "$WORK/bin"
cp "$BIN" "$SWAP_TARGET"
write_manifest "9.9.9" "$CHANNEL/9.9.9/hyprfetch-9.9.9-linux-x64.tar.gz" "$SHA"
OUT=$("$SWAP_TARGET" update -y --channel "$CHANNEL" 2>&1)
echo "$OUT"
echo "$OUT" | grep -q "installed 9.9.9 (sha256 ${SHA:0:16}"
check "reports installed 9.9.9 + sha256 prefix" $?
grep -q "marker-available" "$SWAP_TARGET"
check "binary content was swapped" $?
[ -f "$WORK/bin/hyprfetch.old" ]
check "previous binary kept as .old" $?
[ ! -f "$WORK/bin/hyprfetch.new" ]
check "no .new temp left behind" $?

echo
echo "=== 4. tampered manifest sha256 → refusal, binary untouched ==="
cp "$BIN" "$SWAP_TARGET"
write_manifest "9.9.9" "$CHANNEL/9.9.9/hyprfetch-9.9.9-linux-x64.tar.gz" "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
OUT=$(HYPRFETCH_ASSUME_YES=1 "$SWAP_TARGET" update --channel "$CHANNEL" 2>&1)
RC=$?
echo "$OUT"
[ "$RC" -ne 0 ]
check "command failed" $?
echo "$OUT" | grep -qi "sha256 mismatch"
check "mentions sha256 mismatch" $?
cmp -s "$SWAP_TARGET" "$BIN"
check "binary was NOT swapped" $?

echo
echo "=== 5. unreachable channel → clear error, GitHub never mentioned ==="
OUT=$("$BIN" update --check --channel "http://127.0.0.1:1/" 2>&1)
echo "$OUT"
echo "$OUT" | grep -q "update channel unreachable"
check "reports channel unreachable" $?
echo "$OUT" | grep -q "https://istias.tech/hyprfetch/updates"
check "points at the updates page" $?
echo "$OUT" | grep -q "GitHub is never contacted"
check "says GitHub is never contacted" $?
if echo "$OUT" | grep -v "never contacted" | grep -qiE "github\.com|api\.github|no published release|ls-remote"; then
  check "no GitHub fallback attempt" 1
else
  check "no GitHub fallback attempt" 0
fi

echo
echo "=== 6. --channel '' disables the updater ==="
OUT=$("$BIN" update --check --channel "" 2>&1)
echo "$OUT"
echo "$OUT" | grep -q "updater is disabled"
check "says the updater is disabled" $?

echo
echo "=== 7. web UI /api/update/check answers from the channel ==="
SERVE_DB="$WORK/serve/hyprfetch.db"
mkdir -p "$WORK/serve"
HYPRFETCH_UPDATE_CHANNEL="$CHANNEL" "$BIN" serve --db-path "$SERVE_DB" --bind 127.0.0.1:7789 > "$WORK/serve.log" 2>&1 &
SERVE_PID=$!
UP=1
for _ in $(seq 1 60); do
  curl -sf -o /dev/null http://127.0.0.1:7789/api/server && UP=0 && break
  sleep 0.5
done
check "server came up" $UP
PAYLOAD=$(curl -sf http://127.0.0.1:7789/api/update/check)
echo "payload: $PAYLOAD"
echo "$PAYLOAD" | jq -e '.available == true' > /dev/null
check "available is true" $?
echo "$PAYLOAD" | jq -e '.latest == "9.9.9"' > /dev/null
check "latest is 9.9.9" $?
echo "$PAYLOAD" | jq -e --arg c "$CHANNEL" '.channel == $c' > /dev/null
check "channel echoed back" $?
echo "$PAYLOAD" | jq -e 'has("via_channel") | not' > /dev/null
check "no legacy via_channel/via_git fields" $?
kill "$SERVE_PID" 2>/dev/null; wait "$SERVE_PID" 2>/dev/null; SERVE_PID=""

echo
echo "=== 8. malformed manifest → clean error, no crash ==="
echo "this is not json{{{" > "$WORK/channel/latest.json"
OUT=$("$BIN" update --check --channel "$CHANNEL" 2>&1)
echo "$OUT"
echo "$OUT" | grep -qi "manifest parse\|unreachable"
check "clean error message" $?
if echo "$OUT" | grep -qi "panicked"; then
  check "no panic backtrace" 1
else
  check "no panic backtrace" 0
fi

echo
echo "==============================================="
echo "PASS: $PASS  FAIL: $FAIL"
echo "==============================================="
[ "$FAIL" -eq 0 ]
