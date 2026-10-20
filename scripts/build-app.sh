#!/bin/bash
# Local development bundle. Distribution signing is scripts/build-dist.sh.
set -euo pipefail
cd "$(dirname "$0")/.."
build_source="$(python3 -B scripts/diagnose.py --source-stamp)"
cargo build --locked --offline -p echode-ffi -p echode-plugin-host
bash scripts/build-bundled-plugins.sh
header="$(find target/debug/build -path '*echode-ffi-*/out/echode.h' -print0 | xargs -0 ls -t | head -1)"
test -n "$header"
cp "$header" swift/EchodeBridge/CEchodeFfi/include/echode.h
swift build --package-path swift/EchodeApp --scratch-path target/echode-app
app_bin="$(swift build --package-path swift/EchodeApp --scratch-path target/echode-app --show-bin-path)"
python3 - "$app_bin/Echode" target/debug/echode-plugin-host <<'PY'
import sys
from scripts.distribution import validate_binary
for binary in sys.argv[1:]:
    validate_binary(binary)
PY
app_dir=target/Echode.app
mkdir -p "$app_dir/Contents/MacOS" "$app_dir/Contents/Resources"
cp "$app_bin/Echode" "$app_dir/Contents/MacOS/Echode"
cp target/debug/echode-plugin-host "$app_dir/Contents/MacOS/echode-plugin-host"
mkdir -p "$app_dir/Contents/PlugIns"
cp -R target/debug/plugins/*.clap "$app_dir/Contents/PlugIns/"
cp swift/EchodeApp/Info.plist "$app_dir/Contents/Info.plist"
for app_resources in "$app_bin"/*.bundle; do
    if [[ -d "$app_resources" ]]; then cp -R "$app_resources" "$app_dir/Contents/Resources/"; fi
done
python3 scripts/distribution.py sign-dev --app "$app_dir"
python3 -B scripts/diagnose.py --record-build --expected-source "$build_source"
printf 'Built %s/target/Echode.app\n' "$PWD"
