#!/bin/bash
# Build bundled CLAPs into the same catalog directory used by the app and CLI.
set -euo pipefail
cd "$(dirname "$0")/.."
profile="${1:-debug}"
case "$profile" in
    debug) cargo build -q --locked --offline -p echode-sine-pad ;;
    release) cargo build -q --locked --offline --release -p echode-sine-pad ;;
    *) echo "unsupported profile: $profile" >&2; exit 2 ;;
esac
mkdir -p "target/$profile/plugins"
bundle="target/$profile/plugins/EchodeSinePad.clap"
if [[ -f "$bundle" ]]; then rm "$bundle"; fi
mkdir -p "$bundle/Contents/MacOS"
cp "target/$profile/libechode_sine_pad.dylib" "$bundle/Contents/MacOS/EchodeSinePad"
cp Packaging/EchodeSinePad-Info.plist "$bundle/Contents/Info.plist"
