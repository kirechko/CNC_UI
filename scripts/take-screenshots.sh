#!/usr/bin/env bash
# Take characteristic UI screenshots for the README (task: screenshots into
# README.md). Requires a running backend pair:
#   cargo run --bin cnc-emulator
#   cargo run --bin cnc-web
# then:
#   ./scripts/take-screenshots.sh [base-url]
#
# Preferred engine: Playwright (chromium headless). When it is unavailable the
# script prints a manual capture checklist so the images can be produced by
# hand and dropped into docs/images/ with the expected names.

set -u
BASE_URL="${1:-http://localhost:8080}"
OUT_DIR="$(cd "$(dirname "$0")/.." && pwd)/docs/images"
mkdir -p "$OUT_DIR"

# The shots the README embeds: URL fragment -> output file. Each entry is
# captured at 1440x900 with a short delay so status polling fills the UI.
SHOTS=(
  "/|main-dashboard.png"
  "/#machines|machines.png"
  "/#macros|macro-editor.png"
  "/#program-editor|program-editor.png"
  "/#log|message-log.png"
)

if command -v npx >/dev/null 2>&1 && npx --no-install playwright --version >/dev/null 2>&1; then
  echo "Capturing screenshots with Playwright into $OUT_DIR ..."
  for entry in "${SHOTS[@]}"; do
    fragment="${entry%%|*}"
    file="${entry##*|}"
    npx --no-install playwright screenshot --viewport-size=1440,900 --wait-for-timeout=3500 \
      "${BASE_URL}${fragment}" "$OUT_DIR/$file" || \
      echo "WARN: failed to capture ${BASE_URL}${fragment} -> $file"
  done
  echo "Done. Review $OUT_DIR and uncomment the embeds in README.md."
else
  cat <<EOF
Playwright is not available in this environment.
Capture the screenshots manually (browser window 1440x900, then save into $OUT_DIR):

  1. $BASE_URL          -> main-dashboard.png   (jog dial + 3D viewer + position)
  2. machines dialog    -> machines.png         (discovered machines list)
  3. macro editor       -> macro-editor.png     (open any seeded macro)
  4. program editor     -> program-editor.png   (loaded G-code open for editing)
  5. message log        -> message-log.png      (filters + calendar visible)

After the files exist, uncomment the ![...](docs/images/...) embeds in README.md.
EOF
fi
