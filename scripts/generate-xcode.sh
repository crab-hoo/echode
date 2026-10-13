#!/bin/bash
# Writes Echode.xcodeproj from project.yml. The project is generated, not committed.
set -euo pipefail
cd "$(dirname "$0")/.."
if ! command -v xcodegen >/dev/null; then
    echo "xcodegen is missing. Install it with: brew install xcodegen" >&2
    exit 1
fi
before="$(shasum swift/EchodeApp/Info.plist)"
xcodegen generate --spec project.yml --project .
after="$(shasum swift/EchodeApp/Info.plist)"
if [[ "$before" != "$after" ]]; then
    echo "xcodegen rewrote swift/EchodeApp/Info.plist" >&2
    exit 1
fi
