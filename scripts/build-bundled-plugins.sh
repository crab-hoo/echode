#!/bin/bash
# Build bundled CLAPs into the same catalog directory used by the app and CLI.
# Cargo's default members, as the app and the checks build them: one build (ADR 0328).
set -euo pipefail
cd "$(dirname "$0")/.."
profile="${1:-debug}"
case "$profile" in
    debug) cargo build -q --locked --offline ;;
    release) cargo build -q --locked --offline --release ;;
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
