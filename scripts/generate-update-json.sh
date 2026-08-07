#!/usr/bin/env bash
# Generate latest.json for the Tauri updater.
#
# Usage: ./generate-update-json.sh <tar.gz-path> <sig-path>
#
# After a signed build, Tauri produces a .tar.gz and .tar.gz.sig for the
# updater. This script reads the signature and creates a latest.json that
# the in-app updater checks.
#
# Upload latest.json AND the .tar.gz to the GitHub release.

set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "Usage: $0 <tar.gz-path> <sig-path>" >&2
  exit 1
fi

BUNDLE_PATH="$1"
SIG_PATH="$2"

VERSION=$(grep -o '"version": "[^"]*"' src-tauri/tauri.conf.json | head -1 | cut -d'"' -f4)
BUNDLE_NAME=$(basename "$BUNDLE_PATH")
SIGNATURE=$(cat "$SIG_PATH")
PUB_DATE=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

cat > latest.json << EOF
{
  "version": "$VERSION",
  "notes": "HanhCute v$VERSION",
  "pub_date": "$PUB_DATE",
  "platforms": {
    "darwin-aarch64": {
      "signature": "$SIGNATURE",
      "url": "https://github.com/ductm104/Handy/releases/download/v$VERSION/$BUNDLE_NAME"
    }
  }
}
EOF

echo "Generated latest.json:"
cat latest.json
