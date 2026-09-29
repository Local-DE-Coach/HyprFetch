#!/usr/bin/env bash
# Local simulation of the release-notes generation step from
# .github/workflows/release.yml — verifies the python heredoc logic
# (changelog extraction, sha256 read, body assembly) before tagging.
set -euo pipefail
cd "$(dirname "$0")/.."   # repo root

VERSION="0.4.0"
UPDATES_PAGE="https://istias.tech/hyprfetch/updates"
PRODUCT_PAGE="https://istias.tech/hyprfetch"

# Simulate the .sha256 artifact the release job reads.
rm -rf /tmp/hf_sim_artifacts && mkdir -p /tmp/hf_sim_artifacts
printf '%s  hyprfetch-%s-linux-x64.tar.gz\n' "$(printf 'a%.0s' {1..64})" "$VERSION" \
  > "/tmp/hf_sim_artifacts/hyprfetch-${VERSION}-linux-x64.tar.gz.sha256"

# Extract the release-notes step's run block from the workflow the same way
# the runner executes it: YAML strips the block indentation, bash runs it.
python3 - <<'EXTRACT'
import re
text = open(".github/workflows/release.yml").read()
start = text.index("Generate release notes")
end = text.index("Create / update release")
block = text[start:end]
# Simulate GitHub's ${{ }} expression substitution before bash sees it.
block = re.sub(r"\$\{\{ inputs\.tag \|\| github\.ref_name \}\}", "v0.4.0", block)
m = re.search(r"run: \|\n((?:[ ]{10}.*\n|\n)+)", block)
assert m, "run block not found"
script = "".join(
    (line.rstrip("\n")[10:] if line.rstrip("\n").startswith(" " * 10) else line.rstrip("\n")) + "\n"
    for line in m.group(1).splitlines()
)
open("/tmp/hf_step.sh", "w").write(script)
EXTRACT

cd /tmp/hf_sim_artifacts
VERSION="$VERSION" UPDATES_PAGE="$UPDATES_PAGE" PRODUCT_PAGE="$PRODUCT_PAGE" \
  bash /tmp/hf_step.sh > /tmp/hf_step_out.txt 2>&1 || { tail -20 /tmp/hf_step_out.txt; exit 1; }
echo "=== generated body ==="
cat release_notes.md
echo "=== sanity checks ==="
grep -q "hyprfetch update --check" release_notes.md && echo "OK: update command present"
grep -q "hyprfetch-0.4.0-linux-x64.tar.gz" release_notes.md && echo "OK: clean asset name present"
grep -q "istias.tech/hyprfetch" release_notes.md && echo "OK: product page links present"
