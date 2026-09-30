#!/usr/bin/env bash
# ============================================================================
# v0.4.9 battery — the FINAL update fix (migration instead of escalation).
#
# Every scenario runs against a local mock update channel, as the CURRENT
# (unprivileged) user, with ISOLATED fake HOMEs so nothing touches the real
# profile. What is proven:
#
#   1. Updating a binary in a NON-writable "system" location MIGRATES it to
#      ~/.local/bin (passwordless), fixes PATH for bash/zsh profiles, and —
#      when nothing can relink the old copy — prints the exact one-liner
#      while leaving the old copy UNTOUCHED (self-recovering).
#   2. The NEXT update from ~/.local/bin is a plain in-place swap (silent).
#   3. A RUNNING daemon is restarted FROM THE NEW PATH after a migration.
#   4. `hyprfetch doctor` points at the migration for system-owned installs.
#   5. install.sh: fresh per-user install + idempotent re-run + uninstall.
#   6. install.sh as (namespace-)root: canonical user binary + /usr/local/bin
#      SYMLINK + clean uninstall (needs `unshare -r -m`; skipped when the
#      sandbox forbids user namespaces).
#
# Usage:  scripts/test_v049_battery.sh
# ============================================================================
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release/hyprfetch"
DOCS_INSTALL="${HYPRFETCH_DOCS_INSTALL:-$ROOT/../Docs/hyprfetch/install.sh}"
PORT="${PORT:-8649}"

PASS=0; FAIL=0
check() { # check <name> <rc>
  if [ "$2" = 0 ]; then PASS=$((PASS+1)); echo "  ✓ $1";
  else FAIL=$((FAIL+1)); echo "  ✗ FAIL: $1"; fi
}
wait_http() { # wait_http <url> [tries]
  local tries="${2:-20}"; for _ in $(seq 1 "$tries"); do
    curl -sf "$1" >/dev/null 2>&1 && return 0; sleep 0.4
  done; return 1
}

command -v curl >/dev/null 2>&1 || { echo "need curl"; exit 1; }
[ -x "$BIN" ] || { echo "release binary missing — run: cargo build --release"; exit 1; }

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"; [ -n "${SRV_PID:-}" ] && kill $SRV_PID 2>/dev/null' EXIT
BIN_COPY="$WORK/hyprfetch-0.4.9"; cp "$BIN" "$BIN_COPY"

echo "=== mock update channel (version 9.9.9, payload = the real binary) ==="
PAYLOAD="$WORK/payload/hyprfetch-9.9.9-linux-x64"
mkdir -p "$PAYLOAD"
cp "$BIN" "$PAYLOAD/hyprfetch"; chmod 755 "$PAYLOAD/hyprfetch"
printf 'dummy\n' > "$PAYLOAD/README.md"
printf '[Desktop Entry]\nType=Application\nName=HyprFetch\nExec=hyprfetch\n' > "$PAYLOAD/hyprfetch.desktop"
printf '<svg xmlns="http://www.w3.org/2000/svg"/>\n' > "$PAYLOAD/hyprfetch.svg"
tar czf "$WORK/hyprfetch-9.9.9-linux-x64.tar.gz" -C "$WORK/payload" hyprfetch-9.9.9-linux-x64
SHA=$(sha256sum "$WORK/hyprfetch-9.9.9-linux-x64.tar.gz" | cut -d' ' -f1)
SIZE=$(stat -c%s "$WORK/hyprfetch-9.9.9-linux-x64.tar.gz")
cp "$WORK/hyprfetch-9.9.9-linux-x64.tar.gz" "$WORK/asset.tar.gz"

CHANNEL_DIR="$WORK/channel"
mkdir -p "$CHANNEL_DIR/9.9.9"
cat > "$CHANNEL_DIR/latest.json" <<EOF
{"version":"9.9.9","tag":"v9.9.9","published_at":"2026-10-01T00:00:00Z",
 "assets":{"x86_64-unknown-linux-gnu":{"url":"http://127.0.0.1:$PORT/asset.tar.gz","sha256":"$SHA","size":$SIZE}}}
EOF
# The updater fetches the manifest's URL (/asset.tar.gz); install.sh derives
# <channel>/<version>/<tarball> — serve BOTH layouts from the same channel.
cp "$WORK/asset.tar.gz" "$CHANNEL_DIR/"
cp "$WORK/hyprfetch-9.9.9-linux-x64.tar.gz" "$CHANNEL_DIR/9.9.9/"
printf '%s  hyprfetch-9.9.9-linux-x64.tar.gz\n' "$SHA" \
  > "$CHANNEL_DIR/9.9.9/hyprfetch-9.9.9-linux-x64.tar.gz.sha256"

python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$CHANNEL_DIR" >/dev/null 2>&1 &
SRV_PID=$!
wait_http "http://127.0.0.1:$PORT/latest.json" && check "mock channel is up" 0 || check "mock channel is up" 1

# Shared update env: isolated HOME + NO tty on stdin (never prompt).
# Extra args (VAR=VAL) are appended AFTER `env -i` so overrides survive.
run_update() { # run_update <home> <bin-path> [VAR=VAL...]
  local home="$1"; shift; local bin="$1"; shift
  env -i HOME="$home" PATH=/usr/bin:/bin LANG=C.UTF-8 \
    HYPRFETCH_STATE_DIR="$home/.local/state/hyprfetch" "$@" \
    "$bin" update --yes --channel "http://127.0.0.1:$PORT/" </dev/null 2>&1
}
show_on_fail() { # show_on_fail <rc> <output>
  if [ "$1" != 0 ]; then echo "  ┌─ output ─────────────────────────"; echo "$2" | tail -12; echo "  └──────────────────────────────────"; fi
}

# ============================================================================
echo "=== 1. system-location update MIGRATES to ~/.local/bin (passwordless) ==="
H1="$WORK/home1"; mkdir -p "$H1"
SYSDIR="$WORK/system-bin"; mkdir -p "$SYSDIR"
cp "$BIN" "$SYSDIR/hyprfetch"; chmod 755 "$SYSDIR/hyprfetch"
chmod 555 "$SYSDIR"                       # the "root-owned" trap
OUT=$(run_update "$H1" "$SYSDIR/hyprfetch" "HYPRFETCH_SYSTEM_COPY=$SYSDIR/hyprfetch"); RC=$?
show_on_fail "$RC" "$OUT"
chmod 755 "$SYSDIR"
check "update exits 0 (old code: pkexec failed / needed password)" $RC
echo "$OUT" | grep -q "moved to" && check "reports the migration (moved to …)" 0 || check "reports the migration (moved to …)" 1
[ -x "$H1/.local/bin/hyprfetch" ] && check "binary landed in ~/.local/bin" 0 || check "binary landed in ~/.local/bin" 1
cmp -s "$H1/.local/bin/hyprfetch" "$BIN_COPY" && check "installed binary is the channel payload" 0 || check "installed binary is the channel payload" 1
for RC_FILE in .profile .bashrc .zshrc; do
  grep -q '.local/bin' "$H1/$RC_FILE" 2>/dev/null \
    && check "$RC_FILE gained the guarded PATH block" 0 \
    || check "$RC_FILE gained the guarded PATH block" 1
done
echo "$OUT" | grep -q "sudo rm -f" && check "one-liner printed when relink is impossible" 0 || check "one-liner printed when relink is impossible" 1
cmp -s "$SYSDIR/hyprfetch" "$BIN_COPY" && check "old system copy left UNTOUCHED (self-recovering)" 0 || check "old system copy left UNTOUCHED (self-recovering)" 1
[ -x "$H1/.local/bin/hyprfetch" ] && "$H1/.local/bin/hyprfetch" --version >/dev/null 2>&1 \
  && check "migrated binary runs" 0 || check "migrated binary runs" 1

# ============================================================================
echo "=== 2. the NEXT update from ~/.local/bin is a silent in-place swap ==="
OUT=$(run_update "$H1" "$H1/.local/bin/hyprfetch"); RC=$?
check "second update exits 0" $RC
echo "$OUT" | grep -q "moved to" && check "no second migration (in-place swap)" 1 || check "no second migration (in-place swap)" 0
echo "$OUT" | grep -q "installed 9.9.9" && check "reports installed 9.9.9" 0 || check "reports installed 9.9.9" 1

# ============================================================================
echo "=== 3. a RUNNING daemon restarts FROM THE NEW PATH ==="
H2="$WORK/home2"; mkdir -p "$H2"
SYSDIR2="$WORK/system-bin2"; mkdir -p "$SYSDIR2"
cp "$BIN" "$SYSDIR2/hyprfetch"; chmod 755 "$SYSDIR2/hyprfetch"; chmod 555 "$SYSDIR2"
OUT_START=$(env -i HOME="$H2" PATH=/usr/bin:/bin HYPRFETCH_STATE_DIR="$H2/.local/state/hyprfetch" \
  "$SYSDIR2/hyprfetch" daemon start --bind 127.0.0.1:7831 </dev/null 2>&1); RC=$?
check "daemon starts from the system location" $RC
for _ in $(seq 1 20); do
  curl -sf http://127.0.0.1:7831/healthz >/dev/null 2>&1 && break; sleep 0.4
done
curl -sf http://127.0.0.1:7831/healthz >/dev/null 2>&1 && check "daemon healthz answers" 0 || check "daemon healthz answers" 1
OUT=$(run_update "$H2" "$SYSDIR2/hyprfetch" "HYPRFETCH_SYSTEM_COPY=$SYSDIR2/hyprfetch"); RC=$?
show_on_fail "$RC" "$OUT"
check "update with running daemon exits 0" $RC
echo "$OUT" | grep -Eq "restarting daemon to apply|daemon restarted" \
  && check "CLI noticed the running daemon and restarted it" 0 \
  || check "CLI noticed the running daemon and restarted it" 1
sleep 1.5
NEW_PID=$(sed -n 's/.*"pid": *"\{0,1\}\([0-9]*\)"\{0,1\}.*/\1/p' "$H2/.local/state/hyprfetch/hyprfetch.pid" 2>/dev/null | head -1)
if [ -n "${NEW_PID:-}" ] && [ -e "/proc/$NEW_PID/exe" ]; then
  REAL_EXE=$(readlink -f "/proc/$NEW_PID/exe")
  [ "$REAL_EXE" = "$(readlink -f "$H2/.local/bin/hyprfetch")" ] \
    && check "daemon now runs from ~/.local/bin/hyprfetch (not the old path)" 0 \
    || { check "daemon now runs from ~/.local/bin/hyprfetch (not the old path)" 1; echo "    got: $REAL_EXE"; }
else
  check "daemon pid file present after restart" 1
fi
kill "$NEW_PID" 2>/dev/null; sleep 0.5; kill -9 "$NEW_PID" 2>/dev/null

# ============================================================================
echo "=== 4. doctor points at the migration for system-owned installs ==="
# NOTE: SYSDIR2 is still mode 555 here (the system-owned simulation).
OUT=$(env -i HOME="$H2" PATH=/usr/bin:/bin HYPRFETCH_STATE_DIR="$H2/.local/state/hyprfetch" \
  "$SYSDIR2/hyprfetch" doctor </dev/null 2>&1); RC=$?
check "doctor exits 0" $RC
echo "$OUT" | grep -q "moves hyprfetch to ~/.local/bin" \
  && check "doctor recommends the passwordless migration" 0 \
  || check "doctor recommends the passwordless migration" 1
chmod 755 "$SYSDIR2"

# ============================================================================
echo "=== 5. install.sh: fresh per-user install, re-run, uninstall ==="
if [ ! -f "$DOCS_INSTALL" ]; then
  echo "  (skip: $DOCS_INSTALL not found)"
  SKIP_INSTALLER=1
fi
if [ -z "${SKIP_INSTALLER:-}" ]; then
  sh -n "$DOCS_INSTALL" && check "install.sh syntax (sh -n)" 0 || check "install.sh syntax (sh -n)" 1
  H3="$WORK/home3"; mkdir -p "$H3"
  OUT=$(env -i HOME="$H3" PATH=/usr/bin:/bin LANG=C.UTF-8 \
    HYPRFETCH_CHANNEL="http://127.0.0.1:$PORT" sh "$DOCS_INSTALL" </dev/null 2>&1); RC=$?
  show_on_fail "$RC" "$OUT"
  check "install.sh exits 0" $RC
  [ -x "$H3/.local/bin/hyprfetch" ] && check "canonical binary installed to ~/.local/bin" 0 || check "canonical binary installed to ~/.local/bin" 1
  [ -f "$H3/.local/share/applications/hyprfetch.desktop" ] && check "per-user desktop entry" 0 || check "per-user desktop entry" 1
  [ -f "$H3/.local/share/icons/hicolor/scalable/apps/hyprfetch.svg" ] && check "per-user icon" 0 || check "per-user icon" 1
  grep -q '.local/bin' "$H3/.profile" && check "PATH block written for future shells" 0 || check "PATH block written for future shells" 1
  OUT2=$(env -i HOME="$H3" PATH=/usr/bin:/bin HYPRFETCH_CHANNEL="http://127.0.0.1:$PORT" sh "$DOCS_INSTALL" </dev/null 2>&1); RC2=$?
  check "re-run (update path) exits 0" $RC2
  [ -f "$H3/.local/bin/hyprfetch.old" ] && check "previous binary kept as .old rollback" 0 || check "previous binary kept as .old rollback" 1
  OUT3=$(env -i HOME="$H3" PATH=/usr/bin:/bin sh "$DOCS_INSTALL" --uninstall </dev/null 2>&1); RC3=$?
  check "uninstall exits 0" $RC3
  [ ! -e "$H3/.local/bin/hyprfetch" ] && [ ! -f "$H3/.local/share/applications/hyprfetch.desktop" ] \
    && check "uninstall removed binary + desktop entry" 0 || check "uninstall removed binary + desktop entry" 1

  # ---- 6. install.sh as (namespace-)root: system symlink + uninstall -------
  echo "=== 6. install.sh as root-in-a-user-namespace: system copy → symlink ==="
  if unshare -r -m true >/dev/null 2>&1 && [ -d /usr/local/bin ]; then
    H4="$WORK/home4"; mkdir -p "$H4"
    OUT4=$(unshare -r -m sh -c '
      mount -t tmpfs tmpfs /usr/local/bin 2>/dev/null || exit 95
      printf "OLD" > /usr/local/bin/hyprfetch; chmod 755 /usr/local/bin/hyprfetch
      export HOME="'"$H4"'" PATH=/usr/bin:/bin LANG=C.UTF-8
      HYPRFETCH_CHANNEL="http://127.0.0.1:'"$PORT"'" sh '"$DOCS_INSTALL"' </dev/null
      RC=$?
      echo "INSTALL_RC=$RC"
      [ -x "$HOME/.local/bin/hyprfetch" ] && echo "USER_BIN_OK"
      [ -L /usr/local/bin/hyprfetch ] && echo "SYMLINK_OK"
      [ "$(readlink -f /usr/local/bin/hyprfetch 2>/dev/null)" = "$(readlink -f "$HOME/.local/bin/hyprfetch" 2>/dev/null)" ] && echo "TARGET_OK"
      exit $RC
    ' 2>&1); RC4=$?
    if [ "$RC4" = 95 ]; then
      echo "  (skip: tmpfs mount unavailable in this namespace)"
    else
      [ "$RC4" != 0 ] && { echo "  ┌─ output ──────────────────────"; echo "$OUT4" | tail -12; echo "  └──────────────────────────────"; }
      check "root install exits 0" $RC4
      echo "$OUT4" | grep -q USER_BIN_OK && check "root run installs the canonical USER binary" 0 || check "root run installs the canonical USER binary" 1
      echo "$OUT4" | grep -q SYMLINK_OK && check "/usr/local/bin/hyprfetch is now a symlink" 0 || check "/usr/local/bin/hyprfetch is now a symlink" 1
      echo "$OUT4" | grep -q TARGET_OK && check "symlink points at the canonical binary" 0 || check "symlink points at the canonical binary" 1
      OUT5=$(unshare -r -m sh -c '
        mount -t tmpfs tmpfs /usr/local/bin 2>/dev/null || exit 95
        ln -sf '"$H4"'/.local/bin/hyprfetch /usr/local/bin/hyprfetch
        export HOME="'"$H4"'" PATH=/usr/bin:/bin
        sh '"$DOCS_INSTALL"' --uninstall </dev/null
        if [ -e /usr/local/bin/hyprfetch ] || [ -L /usr/local/bin/hyprfetch ]; then echo STILL_THERE; else echo GONE_OK; fi
      ' 2>&1)
      echo "$OUT5" | grep -q GONE_OK && check "uninstall removed the system symlink" 0 || { check "uninstall removed the system symlink" 1; echo "  ┌─ output ──────────────────────"; echo "$OUT5" | tail -14; echo "  └──────────────────────────────"; }
    fi
  else
    echo "  (skip: unshare/-m or /usr/local/bin unavailable — root path covered by review)"
  fi
fi

kill $SRV_PID 2>/dev/null; wait $SRV_PID 2>/dev/null; SRV_PID=""

echo
echo "==========================================="
echo "v0.4.9 battery: $PASS passed, $FAIL failed"
echo "==========================================="
[ "$FAIL" = 0 ]
