#!/usr/bin/env bash
# build_extension.sh — package the HyprFetch Media Catcher for both stores.
#
# Output (in build/extension/):
#   hyprfetch-extension-<v>-chrome.zip    — Chromium/Brave/Edge (MV3 service worker)
#   hyprfetch-extension-<v>-firefox.xpi   — Firefox (MV3 event page)
#
# The Chrome manifest ships as manifest-chrome.json, the Firefox one as
# manifest-firefox.json; both get renamed to manifest.json inside their
# package. Version is read from manifest-chrome.json.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/extension"
BUILD="$ROOT/build/extension"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

VERSION="$(python3 -c "import json;print(json.load(open('$SRC/manifest-chrome.json'))['version'])")"
CHROME_OUT="$BUILD/hyprfetch-extension-$VERSION-chrome.zip"
FIREFOX_OUT="$BUILD/hyprfetch-extension-$VERSION-firefox.xpi"

mkdir -p "$BUILD"

copy_files() {
    local manifest="$1"
    cp "$SRC/$manifest" "$STAGE/manifest.json"
    cp "$SRC/background.js" "$SRC/popup.html" "$SRC/popup.js" "$SRC/popup.css" "$STAGE/"
    cp -r "$SRC/icons" "$STAGE/icons"
}

echo "==> packaging Chrome (zip)"
rm -f "$CHROME_OUT"
rm -rf "$STAGE" && mkdir -p "$STAGE"
copy_files manifest-chrome.json
(cd "$STAGE" && zip -qr "$CHROME_OUT" .)

echo "==> packaging Firefox (xpi)"
rm -f "$FIREFOX_OUT"
rm -rf "$STAGE" && mkdir -p "$STAGE"
copy_files manifest-firefox.json
(cd "$STAGE" && zip -qr "$FIREFOX_OUT" .)

echo "==> done:"
ls -l "$CHROME_OUT" "$FIREFOX_OUT"

# Sanity: both packages must contain a valid manifest.json.
for f in "$CHROME_OUT" "$FIREFOX_OUT"; do
    python3 - "$f" << 'PY'
import json, sys, zipfile
path = sys.argv[1]
with zipfile.ZipFile(path) as z:
    m = json.loads(z.read("manifest.json"))
    assert m["manifest_version"] == 3, path
    assert "background.js" in z.namelist(), path
    assert "popup.html" in z.namelist(), path
print(f"OK  {path}  (v{m['version']}, {m['background']})")
PY
done
