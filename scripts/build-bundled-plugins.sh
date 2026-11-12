#!/bin/bash
# Build bundled CLAPs into the same catalog directory used by the app and CLI.
set -euo pipefail
cd "$(dirname "$0")/.."
profile="${1:-debug}"
case "$profile" in
    debug) cargo build -q --locked --offline -p echode-sine-pad ;;
    release) cargo build -q --locked --offline --release -p echode-sine-pad ;;
    *)
        echo "unsupported profile: $profile" >&2
        exit 2
        ;;
esac
# Tests find the bundle beside their binaries, also in `make coverage`'s target directory.
out="${CARGO_TARGET_DIR:-target}/$profile"
mkdir -p "$out/plugins"
bundle="$out/plugins/EchodeSinePad.clap"
if [[ -f "$bundle" ]]; then rm "$bundle"; fi
mkdir -p "$bundle/Contents/MacOS"
cp "$out/libechode_sine_pad.dylib" "$bundle/Contents/MacOS/EchodeSinePad"
cp Packaging/EchodeSinePad-Info.plist "$bundle/Contents/Info.plist"
