#!/usr/bin/env bash
# Test the release.yml generator steps locally:
#   1. the python release-notes builder (extracted verbatim from the YAML)
#   2. the Arch PKGBUILD generation (template + sed + bash -n)
set -euo pipefail
cd "$(dirname "$0")/.."   # -> repo root (script lives in scripts/)

VERSION="${1:-0.4.0}"
UPDATES_PAGE="https://istias.tech/hyprfetch/updates"
PRODUCT_PAGE="https://istias.tech/hyprfetch"
OUT="${2:-/tmp/hf-release-test}"
mkdir -p "$OUT"

echo "== 1. extract + run the release-notes python block =="
# Pull the heredoc body between PYEOF markers out of release.yml.
awk '/python3 - <<.PYEOF.$/{flag=1;next}/^          PYEOF$/{flag=0}flag' \
  .github/workflows/release.yml | sed 's/^          //' > "$OUT/notes_gen.py"
# The awk extraction starts one line early (the `python3 -` line is matched
# first and the import block follows) — sanity check the file.
head -3 "$OUT/notes_gen.py"

# Fake the artifacts dir the script reads the sha256 from.
mkdir -p "$OUT/artifacts"
echo "d1ab7d03fa4146d2caaabcbf305f9208cf67cb090305808011e6c397743b5c75  hyprfetch-$VERSION-linux-x64.tar.gz" \
  > "$OUT/artifacts/hyprfetch-$VERSION-linux-x64.tar.gz.sha256"

(cd "$OUT" && VERSION="$VERSION" UPDATES_PAGE="$UPDATES_PAGE" PRODUCT_PAGE="$PRODUCT_PAGE" \
  python3 notes_gen.py > /dev/null)
echo "-- release_notes.md rendered --"
cat "$OUT/release_notes.md"

echo
echo "== 2. PKGBUILD generation =="
SHA=$(cut -d' ' -f1 "$OUT/artifacts/hyprfetch-$VERSION-linux-x64.tar.gz.sha256")
sed -e "s/__VERSION__/${VERSION}/g" -e "s/__SHA256__/${SHA}/" \
  packaging/arch/PKGBUILD.bin.template > "$OUT/PKGBUILD"
bash -n "$OUT/PKGBUILD" && echo "PKGBUILD syntax OK"
cat "$OUT/PKGBUILD"

echo
echo "== 3. sanity assertions =="
grep -q "github.com/Local-DE-Coach/HyprFetch/releases/latest/download/PKGBUILD" "$OUT/release_notes.md" \
  && { echo "FAIL: release notes still point Arch at GitHub"; exit 1; }
grep -q "$UPDATES_PAGE/install.sh" "$OUT/release_notes.md" || { echo "FAIL: install.sh line missing"; exit 1; }
grep -q "$UPDATES_PAGE/$VERSION/PKGBUILD" "$OUT/release_notes.md" || { echo "FAIL: server PKGBUILD command missing"; exit 1; }
grep -q 'source=.*istias.tech/hyprfetch/updates/' "$OUT/PKGBUILD" \
  || { echo "FAIL: PKGBUILD source is not the server"; exit 1; }
grep -q 'github.com' <(grep -E '^(source|url)=' "$OUT/PKGBUILD") \
  && { echo "FAIL: PKGBUILD source/url still references GitHub"; exit 1; }
grep -q 'linux-x64' "$OUT/PKGBUILD" || { echo "FAIL: PKGBUILD _srcdir wrong"; exit 1; }
grep -q 'x86_64-unknown-linux-gnu' "$OUT/PKGBUILD" \
  && { echo "FAIL: PKGBUILD still uses the old triple dir"; exit 1; }
echo "ALL RELEASE-GEN CHECKS PASSED ✓"
