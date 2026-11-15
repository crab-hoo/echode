#!/bin/bash
# Release bundle signed with the Echode Local certificate.
# A missing certificate stops the build before compiling.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ -z "${ECHODE_KEYCHAIN:-}" ]] && ! python3 scripts/distribution.py identity-installed; then
    echo "Echode Local is not in the login keychain. Create it once with: make sign-setup" >&2
    exit 1
fi

# The default members, so the release has the features the checks test (ADR 0328).
cargo build --locked --offline --release
bash scripts/build-bundled-plugins.sh release
header="$(find target/release/build -path '*echode-ffi-*/out/echode.h' -print0 | xargs -0 ls -t | head -1)"
test -n "$header"
cp "$header" swift/EchodeBridge/CEchodeFfi/include/echode.h
mkdir -p target/dist/include
cp "$header" target/dist/include/echode.h
cp swift/EchodeBridge/CEchodeFfi/include/module.modulemap target/dist/include/module.modulemap

export ECHODE_RUST_LIBRARY_DIR="$PWD/target/release"
swift build -c release --product Echode --package-path swift/EchodeApp --scratch-path target/echode-dist
app_bin="$(swift build -c release --product Echode --package-path swift/EchodeApp --scratch-path target/echode-dist --show-bin-path)"

python3 scripts/distribution.py xcframework \
    --library target/release/libechode_ffi.a \
    --headers target/dist/include \
    --output target/dist/EchodeFFI.xcframework

assemble=(
    python3 scripts/distribution.py assemble
    --app target/dist/Echode.app
    --executable "$app_bin/Echode"
    --helper target/release/echode-plugin-host
    --plugin target/release/plugins/EchodeSinePad.clap
    --cli target/release/echode
    --info swift/EchodeApp/Info.plist
    --xcframework target/dist/EchodeFFI.xcframework
)
for bundle in "$app_bin"/*.bundle; do
    if [[ -d "$bundle" ]]; then
        assemble+=(--resource "$bundle")
    fi
done
"${assemble[@]}"
if [[ -n "${ECHODE_KEYCHAIN:-}" ]]; then
    python3 scripts/distribution.py sign --app target/dist/Echode.app --keychain "$ECHODE_KEYCHAIN"
else
    python3 scripts/distribution.py sign --app target/dist/Echode.app
fi

dsym_dir="$(
    python3 scripts/distribution.py archive-dsyms \
        --app target/dist/Echode.app \
        --output-dir target/dist/dSYMs \
        --target Contents/MacOS/Echode "$app_bin/Echode" Echode.app.dSYM \
        --target Contents/MacOS/echode-cli target/release/echode echode-cli.dSYM \
        --target Contents/Helpers/EchodePluginHost.app/Contents/MacOS/echode-plugin-host target/release/echode-plugin-host EchodePluginHost.app.dSYM \
        --target Contents/PlugIns/EchodeSinePad.clap/Contents/MacOS/EchodeSinePad target/release/libechode_sine_pad.dylib EchodeSinePad.clap.dSYM
)"

if [[ -n "${TARGET_BUILD_DIR:-}" && -n "${WRAPPER_NAME:-}" ]]; then
    rm -rf "${TARGET_BUILD_DIR:?}/${WRAPPER_NAME:?}"
    cp -R target/dist/Echode.app "${TARGET_BUILD_DIR}/${WRAPPER_NAME}"
fi
printf 'Built %s/target/dist/Echode.app\n' "$PWD"
printf 'Archived dSYMs in %s\n' "$dsym_dir"
