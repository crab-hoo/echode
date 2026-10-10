#!/bin/bash
# Local development bundle. Distribution signing/XCFrameworks remain separate.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --locked --offline -p echode-ffi
header="$(find target/debug/build -path '*echode-ffi-*/out/echode.h' -print0 | xargs -0 ls -t | head -1)"
test -n "$header"
cp "$header" swift/EchodeBridge/CEchodeFfi/include/echode.h
swift build --package-path swift/EchodeApp --scratch-path target/echode-app
app_bin="$(swift build --package-path swift/EchodeApp --scratch-path target/echode-app --show-bin-path)"
app_dir=target/Echode.app
mkdir -p "$app_dir/Contents/MacOS" "$app_dir/Contents/Resources"
cp "$app_bin/Echode" "$app_dir/Contents/MacOS/Echode"
cp swift/EchodeApp/Info.plist "$app_dir/Contents/Info.plist"
for app_resources in "$app_bin"/*.bundle; do
    if [[ -d "$app_resources" ]]; then cp -R "$app_resources" "$app_dir/Contents/Resources/"; fi
done
codesign --force --sign - "$app_dir"
printf 'Built %s/target/Echode.app\n' "$PWD"
