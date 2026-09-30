#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# HyprFetch Quickshell bar widget — installer
#
# Two modes, one script:
#   1. LOCAL  — run it next to an extracted widget/ tree (release tarball,
#               git checkout): copies ./downloadManager into the
#               illogical-impulse config.
#   2. BOOTSTRAP — run it via curl from the update channel (no files next
#               to it): it downloads the latest widget.tar.gz from
#               istias.tech and re-executes itself in local mode.
#
#   curl -fsSL https://istias.tech/hyprfetch/updates/widget-install.sh | sh
#
# Flags: --uninstall  remove the widget again
# ---------------------------------------------------------------------------
set -euo pipefail

MODE="${1:-install}"
WIDGET_URL="${HYPRFETCH_WIDGET_URL:-https://istias.tech/hyprfetch/updates/widget.tar.gz}"
DEST="${HYPRFETCH_WIDGET_DEST:-$HOME/.config/quickshell/ii/modules/downloadManager}"

if [ "$MODE" = "--uninstall" ]; then
  rm -rf "$DEST"
  echo "Widget removed ($DEST). Reload your shell:  qs -c illogical-impulse kill && qs -c illogical-impulse &"
  exit 0
fi
if [ "$MODE" != "install" ]; then
  echo "usage: $0 [--uninstall]" >&2
  exit 2
fi

SELF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# --- bootstrap mode: fetch the latest widget when run via curl -------------
if [ ! -d "$SELF_DIR/downloadManager" ]; then
  echo "No local widget files found — downloading the latest widget…"
  command -v curl >/dev/null 2>&1 || { echo "curl is required"; exit 1; }
  command -v tar >/dev/null 2>&1 || { echo "tar is required"; exit 1; }
  TMP="$(mktemp -d)"
  trap 'rm -rf "$TMP"' EXIT
  curl -fsSL -o "$TMP/widget.tar.gz" "$WIDGET_URL"
  mkdir -p "$TMP/x"
  tar xzf "$TMP/widget.tar.gz" -C "$TMP/x"
  SELF_DIR="$TMP/x"
  echo "Got the widget from ${WIDGET_URL}"
fi
SRC_DIR="$SELF_DIR/downloadManager"

# --- dependency checks ------------------------------------------------------
# quickshell renders the bar; hyprfetch is the download backend.
# NOTE: aria2 / jq are NOT needed — hyprfetch does the downloading and the
# widget parses JSON itself (zero extra processes).
MISSING=()
command -v quickshell >/dev/null 2>&1 || MISSING+=("quickshell (ships with illogical-impulse)")
command -v hyprfetch  >/dev/null 2>&1 || MISSING+=("hyprfetch (curl -fsSL https://istias.tech/hyprfetch/updates/install.sh | sh)")
if [ "${#MISSING[@]}" -gt 0 ]; then
  echo "Missing dependencies:"
  for m in "${MISSING[@]}"; do echo "  - $m"; done
  echo "Install them first, then re-run this script."
  exit 1
fi

# --- install the QML files ---------------------------------------------------
mkdir -p "$(dirname "$DEST")"
rm -rf "$DEST"
mkdir -p "$DEST"
cp -r "$SRC_DIR/." "$DEST/"
for f in DownloadWidget.qml components/RecentPopup.qml components/InputPopup.qml \
         components/ActivePopup.qml components/CompletionToast.qml utils/DownloadProcess.qml; do
  [ -f "$DEST/$f" ] || { echo "ERROR: $f missing after copy" >&2; exit 1; }
done
echo "Installed widget files to: $DEST"

# --- bar integration ---------------------------------------------------------
BAR="$HOME/.config/quickshell/ii/modules/bar/Bar.qml"
cat <<'EOF'

One manual step is required — add the widget to your bar:

  1. Open  ~/.config/quickshell/ii/modules/bar/Bar.qml
  2. Find the layout section where the other bar modules are instantiated
     (left / center / right — wherever you want the download icon).
  3. Add one line among the other widgets:

         DownloadWidget {}

  4. Reload the shell:

         qs -c illogical-impulse kill && qs -c illogical-impulse &
     (or:  systemctl --user restart qs.service  if you use the systemd unit)

The icon is a download glyph; hovering shows recent downloads, "+" opens
the URL input, live progress appears automatically while anything is
downloading. RAM cost at idle: ~0.5 MB — popups are unloaded when closed.
EOF

if [ -f "$BAR" ] && grep -q "DownloadWidget" "$BAR" 2>/dev/null; then
  echo "Bar already references DownloadWidget — nothing to edit."
elif [ -f "$BAR" ]; then
  echo "Tip: $BAR exists but has no DownloadWidget line yet (step 1–3 above)."
else
  echo "Tip: $BAR not found — add DownloadWidget {} to your bar config wherever it lives."
fi
echo "Done."
