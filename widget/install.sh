#!/bin/sh
# ---------------------------------------------------------------------------
# HyprFetch Quickshell sidebar widget — installer (v0.5.1)
#
# POSIX sh only (sh, dash, bash, zsh, busybox ash all work) so the
# documented one-liner runs anywhere:
#
#   curl -fsSL https://istias.tech/hyprfetch/updates/widget-install.sh | sh
#
# What it does
#   1. removes the OLD v0.5.0 bar widget (module dir + its bar edit) if
#      present — one clean upgrade path
#   2. installs the widget QML into the illogical-impulse sidebar:
#        ~/.config/quickshell/ii/modules/ii/sidebarLeft/downloadManager/
#   3. wires the "Downloads" tab into SidebarLeftContent.qml
#      (surgical, idempotent edits — a .bak-hyprfetch backup is kept)
#   4. sets policies.downloadManager = 1 in ii's config.json
#   5. restarts Quickshell so the tab appears (skip with --no-restart)
#
# Two modes, one script:
#   LOCAL    — run it next to an extracted widget/ tree (release tarball,
#              git checkout): copies ./downloadManager into the config.
#   BOOTSTRAP— run it via curl from the update channel (no files next to
#              it): it downloads the latest widget.tar.gz from istias.tech
#              and installs that.
#
# Flags: --uninstall   remove the widget again (and undo the sidebar edit)
#        --no-restart  don't restart Quickshell automatically
#        Via curl:     curl -fsSL <url> | sh -s -- --uninstall
# ---------------------------------------------------------------------------
set -eu

MODE="install"
NO_RESTART=0
for arg in "$@"; do
  case "$arg" in
    --uninstall) MODE="uninstall" ;;
    --no-restart) NO_RESTART=1 ;;
    -h|--help)
      sed -n '2,30p' "$0" 2>/dev/null || true
      exit 0 ;;
    *)
      printf 'ERROR: unknown flag: %s\n' "$arg" >&2
      printf 'usage: %s [--uninstall] [--no-restart]\n' "$0" >&2
      exit 2 ;;
  esac
done

WIDGET_URL="${HYPRFETCH_WIDGET_URL:-https://istias.tech/hyprfetch/updates/widget.tar.gz}"
QS_ROOT="${HYPRFETCH_QS_ROOT:-$HOME/.config/quickshell/ii}"
SIDEBAR_DIR="$QS_ROOT/modules/ii/sidebarLeft"
DEST="$SIDEBAR_DIR/downloadManager"
SIDEBAR_FILE="$SIDEBAR_DIR/SidebarLeftContent.qml"
II_CONFIG="${HYPRFETCH_II_CONFIG:-$HOME/.config/illogical-impulse/config.json}"
STATUS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/download-manager"
BACKUP_SUFFIX=".bak-hyprfetch"

# Lines the installer adds to SidebarLeftContent.qml (must mirror
# crates/hyprfetch-api/src/widget.rs exactly).
IMPORT_LINE='import "./downloadManager"'
POLICY_PROP='property bool downloadManagerEnabled: Config.options.policies.downloadManager !== 0'
TAB_ENTRY='...(root.downloadManagerEnabled ? [{"icon": "download", "name": Translation.tr("Downloads")}] : [])'
CHILDREN_ENTRY='...(root.downloadManagerEnabled ? [downloadManager.createObject()] : [])'
COMPONENT_LINE='Component { id: downloadManager; DownloadManager {} }'

# Legacy v0.5.0 bar-widget markers (cleaned up on every run).
LEGACY_DIR="$QS_ROOT/modules/downloadManager"
LEGACY_IMPORT="import qs.modules.downloadManager"
LEGACY_MARKER="HyprFetch download widget"

say() { printf '%s\n' "$*"; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Sidebar edit helpers (pure text editing, POSIX awk — no bashisms)
# ---------------------------------------------------------------------------

# Insert the widget import before the first real statement (after the
# import/pragma/comment header).
add_import() {
  awk -v ins="$IMPORT_LINE" '
    BEGIN { done = 0 }
    {
      if (!done && $0 !~ /^[ \t]*$/ && $0 !~ /^[ \t]*\/\// &&
          $0 !~ /^pragma[ \t]/ && $0 !~ /^import[ \t]/) {
        print ins
        done = 1
      }
      print $0
    }
    END { if (!done) print ins }
  ' "$1"
}

# Insert the enabled-flag property right after ii's animeCloset property.
add_policy_prop() {
  awk -v ins="$POLICY_PROP" '
    {
      print $0
      if (!done && $0 ~ /^[ \t]*property bool animeCloset:/) {
        match($0, /^[ \t]*/)
        print substr($0, RSTART, RLENGTH) ins
        done = 1
      }
    }
  ' "$1"
}

# Insert the tab entry just before the `]` that closes tabButtonList
# (indented like the entry above it).
add_tab_entry() {
  awk -v ins="$TAB_ENTRY" '
    {
      if (inlist && !done && $0 ~ /^[ \t]*\][ \t]*$/) {
        match(prev, /^[ \t]*/)
        print substr(prev, RSTART, RLENGTH) ins
        done = 1
      }
      print $0
      prev = $0
      if ($0 ~ /^[ \t]*property var tabButtonList: \[/) inlist = 1
    }
  ' "$1"
}

# Insert the page instance just before the `]` that closes contentChildren.
add_children_entry() {
  awk -v ins="$CHILDREN_ENTRY" '
    {
      if (inlist && !done && $0 ~ /^[ \t]*\][ \t]*$/) {
        match(prev, /^[ \t]*/)
        print substr(prev, RSTART, RLENGTH) ins
        done = 1
      }
      print $0
      prev = $0
      if ($0 ~ /contentChildren: \[/) inlist = 1
    }
  ' "$1"
}

# Insert the one-line Component right after the anime component closes.
add_component() {
  awk -v ins="$COMPONENT_LINE" '
    {
      print $0
      if ($0 ~ /^[ \t]*id: anime[ \t]*$/) inanime = 1
      else if (inanime && !done && $0 ~ /^[ \t]*}[ \t]*$/) {
        match($0, /^[ \t]*/)
        print substr($0, RSTART, RLENGTH) ins
        done = 1
        inanime = 0
      }
    }
  ' "$1"
}

# Undo all five insertions (line-prefix removal, mirrors uninstall).
# Byte-exact: a file that had no trailing newline keeps having none.
remove_sidebar_edits() {
  if [ -s "$1" ] && [ -z "$(tail -c 1 "$1" 2>/dev/null)" ]; then NL=1; else NL=0; fi
  awk -v finalnl="$NL" '
    BEGIN { first = 1 }
    $0 ~ /^[ \t]*import "\.\/downloadManager"/ { next }
    $0 ~ /^[ \t]*property bool downloadManagerEnabled:/ { next }
    index($0, "...(root.downloadManagerEnabled ? [{\"icon\": \"download\"") > 0 { next }
    index($0, "...(root.downloadManagerEnabled ? [downloadManager.createObject()]") > 0 { next }
    index($0, "Component { id: downloadManager; DownloadManager {} }") > 0 { next }
    {
      if (first) { first = 0 } else { printf "\n" }
      printf "%s", $0
    }
    END { if (finalnl) printf "\n" }
  ' "$1"
}

# Emit $1 on stdout with the trailing-newline state forced to finalnl
# (1 = ends with \n, 0 = does not). Keeps edits byte-exact.
normalize_nl() {
  if [ -s "$1" ] && [ -z "$(tail -c 1 "$1" 2>/dev/null)" ]; then CUR=1; else CUR=0; fi
  if [ "$CUR" = "$2" ]; then cat "$1"; return; fi
  if [ "$CUR" = "1" ] && [ "$2" = "0" ]; then
    SIZE=$(wc -c < "$1")
    head -c $((SIZE - 1)) "$1"
  else
    cat "$1"
    printf '\n'
  fi
}

sidebar_is_integrated() {
  grep -q "Component { id: downloadManager; DownloadManager {} }" "$SIDEBAR_FILE" 2>/dev/null
}

# Wire the Downloads tab. Idempotent; backs up once; unknown layouts are
# left alone with printed manual instructions.
integrate_sidebar() {
  if [ ! -f "$SIDEBAR_FILE" ]; then
    say "NOTE: $SIDEBAR_FILE not found — widget files are installed,"
    say "      but add the Downloads tab to your sidebar manually (see README)."
    return 0
  fi
  if sidebar_is_integrated; then
    say "Sidebar integration: already present in $SIDEBAR_FILE"
    return 0
  fi
  # Anchor check: only edit files we understand.
  if ! grep -q "tabButtonList" "$SIDEBAR_FILE" || \
     ! grep -q "contentChildren" "$SIDEBAR_FILE" || \
     ! grep -q "property bool animeCloset:" "$SIDEBAR_FILE"; then
    say "NOTE: could not auto-edit $SIDEBAR_FILE (unknown layout)."
    say "      Add these lines yourself:"
    say "        $IMPORT_LINE                      (with the imports)"
    say "        $POLICY_PROP"
    say "        $TAB_ENTRY            (inside tabButtonList)"
    say "        $CHILDREN_ENTRY  (inside contentChildren)"
    say "        $COMPONENT_LINE   (next to the other Components)"
    return 0
  fi
  [ -f "$SIDEBAR_FILE$BACKUP_SUFFIX" ] || cp "$SIDEBAR_FILE" "$SIDEBAR_FILE$BACKUP_SUFFIX"
  # Remember the original trailing-newline state — edits must not change it.
  # (1 = file ends with \n, 0 = it does not — same convention as normalize_nl)
  if [ -s "$SIDEBAR_FILE" ] && [ -z "$(tail -c 1 "$SIDEBAR_FILE" 2>/dev/null)" ]; then
    ORIGNL=1
  else
    ORIGNL=0
  fi
  TMP="$SIDEBAR_FILE.hyprfetch-tmp"
  add_import "$SIDEBAR_FILE" > "$TMP.1"
  add_policy_prop "$TMP.1" > "$TMP.2"
  add_tab_entry "$TMP.2" > "$TMP.3"
  add_children_entry "$TMP.3" > "$TMP.4"
  add_component "$TMP.4" > "$TMP.5"
  normalize_nl "$TMP.5" "$ORIGNL" > "$TMP.6"
  # Verify the edits actually landed before swapping the file in.
  if grep -q "Component { id: downloadManager; DownloadManager {} }" "$TMP.6" && \
     grep -q "downloadManagerEnabled" "$TMP.6"; then
    mv "$TMP.6" "$SIDEBAR_FILE"
    say "Sidebar integration: added the Downloads tab to $SIDEBAR_FILE"
    say "  (original saved as $SIDEBAR_FILE$BACKUP_SUFFIX)"
  else
    say "NOTE: could not auto-edit $SIDEBAR_FILE safely — file left untouched."
    say "      Add these lines yourself:"
    say "        $IMPORT_LINE"
    say "        $POLICY_PROP"
    say "        $TAB_ENTRY"
    say "        $CHILDREN_ENTRY"
    say "        $COMPONENT_LINE"
  fi
  rm -f "$TMP" "$TMP.1" "$TMP.2" "$TMP.3" "$TMP.4" "$TMP.5" "$TMP.6"
}

# ---------------------------------------------------------------------------
# Legacy v0.5.0 bar-widget cleanup
# ---------------------------------------------------------------------------

remove_legacy_bar_edit() {
  awk -v marker="$LEGACY_MARKER" '
    $0 ~ /^[ \t]*import qs[.]modules[.]downloadManager/ { next }
    index($0, "// " marker " (auto-added") > 0 { inblock = 1; guard = 0; next }
    inblock {
      if ($0 ~ /^[ \t]*}[ \t]*$/) { inblock = 0; next }  # consume the block close too
      else if (++guard > 12) { inblock = 0 }              # safety: never eat the whole file
      else { next }
    }
    { print $0 }
  ' "$1"
}

cleanup_legacy() {
  CLEANED=0
  if [ -d "$LEGACY_DIR" ]; then
    rm -rf "$LEGACY_DIR"
    say "Removed the old bar widget ($LEGACY_DIR)."
    CLEANED=1
  fi
  for BAR in \
    "$QS_ROOT/modules/ii/bar/BarContent.qml" \
    "$QS_ROOT/modules/bar/BarContent.qml" \
    "$QS_ROOT/modules/ii/bar/Bar.qml" \
    "$QS_ROOT/modules/bar/Bar.qml"
  do
    [ -f "$BAR" ] || continue
    if grep -q "// $LEGACY_MARKER (auto-added" "$BAR" 2>/dev/null || \
       grep -q "^$LEGACY_IMPORT" "$BAR" 2>/dev/null; then
      [ -f "$BAR$BACKUP_SUFFIX" ] || cp "$BAR" "$BAR$BACKUP_SUFFIX"
      TMP_BAR="$BAR.hyprfetch-tmp"
      remove_legacy_bar_edit "$BAR" > "$TMP_BAR"
      mv "$TMP_BAR" "$BAR"
      say "Removed the old widget from your bar ($BAR)."
      CLEANED=1
    fi
    break
  done
}

# ---------------------------------------------------------------------------
# Uninstall
# ---------------------------------------------------------------------------
if [ "$MODE" = "uninstall" ]; then
  REMOVED=0
  if [ -d "$DEST" ]; then
    rm -rf "$DEST"
    say "Widget files removed ($DEST)."
    REMOVED=1
  fi
  if [ -f "$SIDEBAR_FILE" ] && grep -q "downloadManager" "$SIDEBAR_FILE" 2>/dev/null; then
    [ -f "$SIDEBAR_FILE$BACKUP_SUFFIX" ] || cp "$SIDEBAR_FILE" "$SIDEBAR_FILE$BACKUP_SUFFIX"
    TMP="$SIDEBAR_FILE.hyprfetch-tmp"
    remove_sidebar_edits "$SIDEBAR_FILE" > "$TMP"
    mv "$TMP" "$SIDEBAR_FILE"
    say "Downloads tab removed from $SIDEBAR_FILE."
    REMOVED=1
  fi
  cleanup_legacy
  [ "$REMOVED" = "1" ] || say "Nothing to remove — widget was not installed."
  say "Reload your shell to apply:  qs -c ii kill; qs -c ii &"
  exit 0
fi

# ---------------------------------------------------------------------------
# Locate the widget sources: local tree or download from the channel
# ---------------------------------------------------------------------------
SRC_DIR=""
if [ -f "$(pwd)/downloadManager/DownloadManager.qml" ]; then
  SRC_DIR="$(pwd)/downloadManager"
elif [ -f "$0" ] && [ "$0" != "sh" ] && [ "$0" != "bash" ] && [ "$0" != "dash" ] \
     && [ "$0" != "-" ] && [ "$0" != "--" ]; then
  # Executed as a real file — look next to it (LOCAL mode).
  SELF_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" 2>/dev/null && pwd -P) || SELF_DIR=""
  if [ -n "$SELF_DIR" ] && [ -f "$SELF_DIR/downloadManager/DownloadManager.qml" ]; then
    SRC_DIR="$SELF_DIR/downloadManager"
  fi
fi

if [ -z "$SRC_DIR" ]; then
  # BOOTSTRAP mode (the `curl | sh` path).
  say "Downloading the latest widget from the update channel…"
  command -v curl >/dev/null 2>&1 || die "curl is required (or download widget.tar.gz and run install.sh next to it)"
  command -v tar  >/dev/null 2>&1 || die "tar is required"
  TMP="$(mktemp -d)" || die "mktemp failed"
  trap 'rm -rf "$TMP"' EXIT INT TERM
  curl -fsSL -o "$TMP/widget.tar.gz" "$WIDGET_URL" || die "could not download $WIDGET_URL"
  mkdir -p "$TMP/x"
  tar xzf "$TMP/widget.tar.gz" -C "$TMP/x" || die "could not extract the widget archive"
  [ -f "$TMP/x/downloadManager/DownloadManager.qml" ] || die "archive does not contain downloadManager/DownloadManager.qml"
  SRC_DIR="$TMP/x/downloadManager"
  say "Got the widget from ${WIDGET_URL}"
fi

# ---------------------------------------------------------------------------
# Dependency checks
# ---------------------------------------------------------------------------
# quickshell renders the shell; hyprfetch is the download backend.
# NOTE: aria2 / jq are NOT needed — hyprfetch does the downloading and the
# widget parses JSON itself.
MISSING=""
command -v quickshell >/dev/null 2>&1 || MISSING="$MISSING
  - quickshell (ships with illogical-impulse)"
command -v hyprfetch >/dev/null 2>&1 || MISSING="$MISSING
  - hyprfetch (curl -fsSL https://istias.tech/hyprfetch/updates/install.sh | sh)"
if [ -n "$MISSING" ]; then
  say "Missing dependencies:$MISSING"
  say "Install them first, then re-run this script."
  exit 1
fi

# ---------------------------------------------------------------------------
# Install the QML files
# ---------------------------------------------------------------------------
[ -d "$QS_ROOT" ] || die "illogical-impulse quickshell config not found at $QS_ROOT — install ii first."
[ -d "$SIDEBAR_DIR" ] || die "ii sidebar not found at $SIDEBAR_DIR — is your ii checkout complete?"

say "==> Upgrading: removing the old v0.5.0 bar widget if present…"
cleanup_legacy

say "==> Installing the Downloads tab widget…"
mkdir -p "$(dirname "$DEST")"
rm -rf "$DEST"
mkdir -p "$DEST/components"
for f in DownloadManager.qml components/DownloadHeader.qml components/DownloadList.qml \
         components/DownloadItem.qml components/DownloadInputBar.qml; do
  cp "$SRC_DIR/$f" "$DEST/$f" || die "could not copy $f"
done
for f in DownloadManager.qml components/DownloadHeader.qml components/DownloadList.qml \
         components/DownloadItem.qml components/DownloadInputBar.qml; do
  [ -f "$DEST/$f" ] || die "$f missing after copy"
done
say "Installed widget files to: $DEST"

# ---------------------------------------------------------------------------
# Wire the Downloads tab into the sidebar (automatic — no manual step)
# ---------------------------------------------------------------------------
integrate_sidebar

# ---------------------------------------------------------------------------
# Set policies.downloadManager = 1 in ii's config.json (idempotent)
# ---------------------------------------------------------------------------
if command -v python3 >/dev/null 2>&1; then
  python3 - "$II_CONFIG" <<'PYEOF'
import json, sys
config_path = sys.argv[1]
try:
    with open(config_path, 'r') as f:
        config = json.load(f)
except FileNotFoundError:
    print("==> ii config.json not found — the Downloads tab uses its default (enabled)")
    sys.exit(0)
except Exception as e:
    print(f"==> Could not read ii config: {e} — the tab still uses its default (enabled)")
    sys.exit(0)
policies = config.setdefault('policies', {})
if policies.get('downloadManager') == 1:
    print("==> 'downloadManager' policy already enabled in config.json")
else:
    policies['downloadManager'] = 1
    with open(config_path, 'w') as f:
        json.dump(config, f, indent=4)
    print("==> Enabled 'downloadManager' policy in config.json")
PYEOF
else
  say "NOTE: python3 not found — add \"downloadManager\": 1 to \"policies\" in $II_CONFIG yourself."
  say "      (Not required — the tab defaults to enabled.)"
fi

# ---------------------------------------------------------------------------
# Initial status file so the tab opens with valid JSON on first launch
# ---------------------------------------------------------------------------
if [ ! -f "$STATUS_DIR/status.json" ]; then
  mkdir -p "$STATUS_DIR"
  printf '{"active_downloads":[],"recent_downloads":[],"last_completed":null}\n' > "$STATUS_DIR/status.json"
  say "==> Created initial status file ($STATUS_DIR/status.json)"
fi

# ---------------------------------------------------------------------------
# Restart Quickshell so the tab appears
# ---------------------------------------------------------------------------
if [ "$NO_RESTART" = "1" ]; then
  say ""
  say "Skipping restart (--no-restart). Reload the shell to see the tab:"
  say "  qs -c ii kill; qs -c ii &"
else
  say ""
  say "==> Restarting Quickshell…"
  if command -v systemctl >/dev/null 2>&1 && \
     systemctl --user is-active --quiet quickshell.service 2>/dev/null; then
    systemctl --user restart quickshell.service || true
    say "Restarted via quickshell.service."
  else
    pkill qs 2>/dev/null || true
    sleep 1
    (nohup qs -c ii >/dev/null 2>&1 &) || true
    say "Quickshell relaunched."
  fi
fi

say ""
say "========================================="
say " Installation complete!"
say "========================================="
say " Look for the 'Downloads' tab in your left sidebar."
say " Paste a URL, confirm the save path, and it downloads."
say " Uninstall any time:"
say "   curl -fsSL https://istias.tech/hyprfetch/updates/widget-install.sh | sh -s -- --uninstall"
say "========================================="
