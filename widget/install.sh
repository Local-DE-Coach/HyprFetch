#!/bin/sh
# ---------------------------------------------------------------------------
# HyprFetch Quickshell bar widget — installer
#
# POSIX sh only (sh, dash, bash, zsh, busybox ash all work) so the
# documented one-liner runs anywhere:
#
#   curl -fsSL https://istias.tech/hyprfetch/updates/widget-install.sh | sh
#
# Two modes, one script:
#   1. LOCAL    — run it next to an extracted widget/ tree (release tarball,
#                 git checkout): copies ./downloadManager into the
#                 illogical-impulse config.
#   2. BOOTSTRAP— run it via curl from the update channel (no files next
#                 to it): it downloads the latest widget.tar.gz from
#                 istias.tech and installs that.
#
# The installer also wires the widget into the ii bar automatically:
# it edits the bar QML (with a .bak-hyprfetch backup) so no manual step
# is needed; if the layout is unknown it prints the two lines to add.
#
# Flags: --uninstall  remove the widget again (and undo the bar edit).
#        Run via curl with:  curl -fsSL <url> | sh -s -- --uninstall
# ---------------------------------------------------------------------------
set -eu

MODE="${1:-install}"
WIDGET_URL="${HYPRFETCH_WIDGET_URL:-https://istias.tech/hyprfetch/updates/widget.tar.gz}"
QS_ROOT="${HYPRFETCH_QS_ROOT:-$HOME/.config/quickshell/ii}"
DEST="$QS_ROOT/modules/downloadManager"
IMPORT_LINE="import qs.modules.downloadManager"
MARKER="HyprFetch download widget"
BACKUP_SUFFIX=".bak-hyprfetch"

say() { printf '%s\n' "$*"; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Bar-file helpers (pure text editing, no bashisms)
# ---------------------------------------------------------------------------

# First bar QML that exists in this ii checkout.
find_bar_file() {
  for f in \
    "$QS_ROOT/modules/ii/bar/BarContent.qml" \
    "$QS_ROOT/modules/bar/BarContent.qml" \
    "$QS_ROOT/modules/ii/bar/Bar.qml" \
    "$QS_ROOT/modules/bar/Bar.qml"
  do
    if [ -f "$f" ]; then
      printf '%s\n' "$f"
      return 0
    fi
  done
  return 1
}

# Insert the module import before the first real statement (after the
# import/pragma/comment header), wherever that is.
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

# Insert the widget instance after the right-side RowLayout anchor
# (layoutDirection: Qt.RightToLeft) so the icon lands at the far right
# of the bar, next to the system indicators. Honors the file indent.
add_widget_block() {
  awk -v marker="$MARKER" '
    {
      print $0
      if (!done && $0 ~ /^[ \t]*layoutDirection:.*Qt\.RightToLeft/) {
        match($0, /^[ \t]*/)
        indent = substr($0, RSTART, RLENGTH)
        print indent "// " marker " (auto-added — delete this block to remove)"
        print indent "DownloadWidget {"
        print indent "    Layout.alignment: Qt.AlignVCenter"
        print indent "}"
        done = 1
      }
    }
  ' "$1"
}

# Undo add_import + add_widget_block. Prints the cleaned file to stdout.
remove_widget_edit() {
  awk -v marker="$MARKER" '
    $0 ~ /^import qs[.]modules[.]downloadManager/ { next }
    index($0, "// " marker " (auto-added") > 0 { inblock = 1; guard = 0; next }
    inblock {
      if ($0 ~ /^[ \t]*}[ \t]*$/) { inblock = 0; next }  # consume the block close too
      else if (++guard > 12) { inblock = 0 }              # safety: never eat the whole file
      else { next }
    }
    { print $0 }
  ' "$1"
}

bar_is_integrated() {
  grep -q "DownloadWidget" "$1" 2>/dev/null
}

# Integrate the widget into the bar file. Idempotent; backs up once.
integrate_bar() {
  BAR="$(find_bar_file)" || {
    say "NOTE: no ii bar QML found under $QS_ROOT/modules — the widget files are installed,"
    say "      but you must add it to your bar yourself:"
    say "        1. import qs.modules.downloadManager"
    say "        2. DownloadWidget {}   (inside your bar layout row)"
    return 0
  }
  if bar_is_integrated "$BAR"; then
    say "Bar integration: already present in $BAR"
    return 0
  fi
  # Anchor check first: only edit files we understand.
  if ! grep -q "layoutDirection:.*Qt.RightToLeft" "$BAR" 2>/dev/null; then
    say "NOTE: could not auto-edit $BAR (unknown layout)."
    say "      Add these two lines yourself:"
    say "        import qs.modules.downloadManager"
    say "        DownloadWidget {}   (inside your bar layout row)"
    return 0
  fi
  [ -f "$BAR$BACKUP_SUFFIX" ] || cp "$BAR" "$BAR$BACKUP_SUFFIX"
  TMP_BAR="$BAR.hyprfetch-tmp"
  add_import "$BAR" > "$TMP_BAR"
  add_widget_block "$TMP_BAR" > "$TMP_BAR.2"
  mv "$TMP_BAR.2" "$BAR"
  rm -f "$TMP_BAR"
  say "Bar integration: added DownloadWidget to $BAR"
  say "  (original saved as $BAR$BACKUP_SUFFIX)"
}

# ---------------------------------------------------------------------------
# Uninstall
# ---------------------------------------------------------------------------
if [ "$MODE" = "--uninstall" ]; then
  REMOVED=0
  if [ -d "$DEST" ]; then
    rm -rf "$DEST"
    say "Widget files removed ($DEST)."
    REMOVED=1
  fi
  if BAR="$(find_bar_file)" && grep -q "// $MARKER (auto-added" "$BAR" 2>/dev/null; then
    [ -f "$BAR$BACKUP_SUFFIX" ] || cp "$BAR" "$BAR$BACKUP_SUFFIX"
    TMP_BAR="$BAR.hyprfetch-tmp"
    remove_widget_edit "$BAR" > "$TMP_BAR"
    mv "$TMP_BAR" "$BAR"
    say "Bar integration removed from $BAR."
    REMOVED=1
  fi
  [ "$REMOVED" = "1" ] || say "Nothing to remove — widget was not installed."
  say "Reload your shell to apply:  qs -c ii kill; qs -c ii &"
  exit 0
fi
if [ "$MODE" != "install" ]; then
  printf 'usage: %s [--uninstall]\n' "$0" >&2
  exit 2
fi

# ---------------------------------------------------------------------------
# Locate the widget sources: local tree or download from the channel
# ---------------------------------------------------------------------------
SRC_DIR=""
if [ -d "$(pwd)/downloadManager" ] && [ -f "$(pwd)/downloadManager/DownloadWidget.qml" ]; then
  SRC_DIR="$(pwd)/downloadManager"
elif [ -f "$0" ] && [ "$0" != "sh" ] && [ "$0" != "bash" ] && [ "$0" != "dash" ]; then
  # Executed as a real file — look next to it (LOCAL mode).
  SELF_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" 2>/dev/null && pwd -P) || SELF_DIR=""
  if [ -n "$SELF_DIR" ] && [ -f "$SELF_DIR/downloadManager/DownloadWidget.qml" ]; then
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
  [ -f "$TMP/x/downloadManager/DownloadWidget.qml" ] || die "archive does not contain downloadManager/DownloadWidget.qml"
  SRC_DIR="$TMP/x/downloadManager"
  say "Got the widget from ${WIDGET_URL}"
fi

# ---------------------------------------------------------------------------
# Dependency checks
# ---------------------------------------------------------------------------
# quickshell renders the bar; hyprfetch is the download backend.
# NOTE: aria2 / jq are NOT needed — hyprfetch does the downloading and the
# widget parses JSON itself (zero extra processes).
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
mkdir -p "$(dirname "$DEST")"
rm -rf "$DEST"
mkdir -p "$DEST"
cp -r "$SRC_DIR/." "$DEST/"
for f in DownloadWidget.qml components/RecentPopup.qml components/InputPopup.qml \
         components/ActivePopup.qml components/CompletionToast.qml utils/DownloadProcess.qml; do
  [ -f "$DEST/$f" ] || die "$f missing after copy"
done
say "Installed widget files to: $DEST"

# ---------------------------------------------------------------------------
# Bar integration (automatic — no manual step)
# ---------------------------------------------------------------------------
integrate_bar

say ""
say "Reload the shell to see it:"
say "  qs -c ii kill; qs -c ii &"
say "  (or: systemctl --user restart quickshell.service, if you use the unit)"
say ""
say "The icon is a download glyph; hovering shows recent downloads, clicking"
say "opens the URL input, live progress appears automatically while anything"
say "is downloading. RAM cost at idle: ~0.5 MB — popups are unloaded when closed."
say "Done."
