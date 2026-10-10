#!/usr/bin/env bash
# UI-2: regenerate both ABIs and test unmodified generated Swift under Swift 6.
set -euo pipefail
cd "$(dirname "$0")/.."
bridge_root="$PWD"
bridge_output="$bridge_root/target/bridge"
bridge_library="$bridge_root/target/release"
mkdir -p "$bridge_output/c" "$bridge_output/uniffi" "$bridge_output/uniffi-async"
export CARGO_TARGET_DIR="$bridge_root/target"
export DYLD_LIBRARY_PATH="$bridge_library${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}"

swift --version > "$bridge_output/report.txt"
rustc --version >> "$bridge_output/report.txt"
cat "$bridge_output/report.txt"

cargo build --locked --offline --release -p echode-bridge-spike --features bindgen,fault-injection
"$bridge_library/echode-bridge-bindgen" c-header "$bridge_output/c/echode_bridge.h"
cp spikes/bridge/c/module.modulemap "$bridge_output/c/module.modulemap"
"$bridge_library/echode-bridge-bindgen" generate "$bridge_library/libechode_bridge_spike.dylib" \
    --language swift --out-dir "$bridge_output/uniffi"

# Put the compiler cache in the workspace, including inside the filesystem sandbox.
swift_flags=(-O -swift-version 6 -strict-concurrency=complete -warnings-as-errors \
    -target arm64-apple-macos27.0 \
    -parse-as-library -module-cache-path "$bridge_output/module-cache" \
    -L "$bridge_library" -lechode_bridge_spike)
swiftc "${swift_flags[@]}" -I "$bridge_output/c" -D BRIDGE_VALIDATION \
    -Xcc -DECHODE_BRIDGE_FAULT_INJECTION \
    spikes/bridge/swift/CBridge.swift spikes/bridge/swift/CProbe.swift spikes/bridge/swift/ContractProbe.swift \
    -o "$bridge_output/c-probe"
python3 scripts/run-bridge-validation.py --log "$bridge_output/c-probe.log" \
    "$bridge_output/c-probe" "$@" | tee -a "$bridge_output/report.txt"

swiftc "${swift_flags[@]}" -I "$bridge_output/uniffi" \
    -Xcc "-fmodule-map-file=$bridge_output/uniffi/UniBridgeFFI.modulemap" \
    "$bridge_output/uniffi/UniBridge.swift" spikes/bridge/swift/UniProbe.swift \
    -o "$bridge_output/uniffi-probe"
python3 scripts/run-bridge-validation.py --log "$bridge_output/uniffi-probe.log" \
    "$bridge_output/uniffi-probe" "$@" | tee -a "$bridge_output/report.txt"

# Async foreign callbacks are a separate probe; a failure must match the upstream
# Swift concurrency diagnostic, never a linker/configuration error or test crash.
cargo build --locked --offline --release -p echode-bridge-spike --features bindgen,fault-injection,async-callback
"$bridge_library/echode-bridge-bindgen" generate "$bridge_library/libechode_bridge_spike.dylib" \
    --language swift --out-dir "$bridge_output/uniffi-async"
if swiftc "${swift_flags[@]}" -I "$bridge_output/uniffi-async" \
    -Xcc "-fmodule-map-file=$bridge_output/uniffi-async/UniBridgeFFI.modulemap" \
    -D ASYNC_CALLBACK "$bridge_output/uniffi-async/UniBridge.swift" \
    spikes/bridge/swift/UniProbe.swift -o "$bridge_output/uniffi-async-probe" \
    > "$bridge_output/uniffi-async.log" 2>&1; then
    python3 scripts/run-bridge-validation.py --log "$bridge_output/uniffi-async-run.log" \
        "$bridge_output/uniffi-async-probe" | tee -a "$bridge_output/report.txt"
else
    # Reject failures outside the generated file and any additional error kinds.
    errors=$(rg ': error:' "$bridge_output/uniffi-async.log")
    if [ -z "$errors" ] || printf '%s\n' "$errors" | \
        rg -v '/uniffi-async/UniBridge.swift:.*SendingClosureRisksDataRace' >/dev/null; then
        cat "$bridge_output/uniffi-async.log"
        exit 1
    fi
    printf '%s\n' 'UniFFI async foreign callback: EXPECTED compiler rejection (SendingClosureRisksDataRace)' \
        | tee -a "$bridge_output/report.txt"
fi

# Leave the normal library matching the normal generated bindings, even after
# the intentionally unsupported callback probe.
cargo build --locked --offline --release -p echode-bridge-spike --features bindgen,fault-injection
