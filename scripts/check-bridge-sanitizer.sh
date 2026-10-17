#!/usr/bin/env bash
# UI-2: TSan instruments Rust, dependencies/std, and Swift with one Apple runtime.
# AddressSanitizer dispatches to its explicitly separate-language checks.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/bridge-toolchain.env
bridge_sanitizer="${1:?usage: check-bridge-sanitizer.sh address|thread}"
case "$bridge_sanitizer" in
    address) exec bash scripts/check-bridge-address.sh ;;
    thread) ;;
    *) exit 2 ;;
esac
bridge_root="$PWD"
bridge_output="$bridge_root/target/bridge/$bridge_sanitizer"
bridge_target=aarch64-apple-darwin
mkdir -p "$bridge_output/c" "$bridge_output/control"
swift --version > "$bridge_output/versions.txt"
rustc "+$BRIDGE_NIGHTLY" -vV >> "$bridge_output/versions.txt"

# The generator is a host tool, built without sanitizer instrumentation.
cargo build --locked --offline --release -p echode-bridge-spike --features bindgen,fault-injection
"$bridge_root/target/release/echode-bridge-bindgen" c-header "$bridge_output/c/echode_bridge.h"
cp spikes/bridge/c/module.modulemap "$bridge_output/c/module.modulemap"
"$bridge_root/target/release/echode-bridge-bindgen" sanitizer-header "$bridge_output/control/probe.h"
cat > "$bridge_output/control/module.modulemap" <<'MODULE'
module CBridgeSanitizerProbe { header "probe.h" export * }
MODULE

# --target keeps host build scripts/macros uninstrumented. build-std is essential
# for TSan to observe Rust's synchronization, including Arc and the command queue.
export CARGO_TARGET_DIR="$bridge_output/rust"
export RUSTFLAGS="-Ctarget-cpu=apple-m4 -Zsanitizer=$bridge_sanitizer -Zexternal-clangrt -Cdebuginfo=2 -Cforce-frame-pointers=yes"
cargo "+$BRIDGE_NIGHTLY" rustc --locked --offline -Zbuild-std --target "$bridge_target" \
    --release --lib --crate-type staticlib -p echode-bridge-spike --features fault-injection
bridge_library="$CARGO_TARGET_DIR/$bridge_target/release"
cp "$bridge_library/libechode_bridge_spike.a" "$bridge_output/c/libbridge.a"

# Separate fixture library: no deliberate fault is linked into the actual probe.
cargo "+$BRIDGE_NIGHTLY" rustc --locked --offline -Zbuild-std --target "$bridge_target" \
    --release --lib --crate-type staticlib -p echode-bridge-spike --features fault-injection,sanitizer-control
cp "$bridge_library/libechode_bridge_spike.a" "$bridge_output/control/libcontrol.a"
swift_flags=("${ECHODE_SWIFT_CPU_FLAGS[@]}" -swift-version 6 -strict-concurrency=complete -warnings-as-errors \
    -target arm64-apple-macos27.0 -parse-as-library -g -sanitize="$bridge_sanitizer" \
    -module-cache-path "$bridge_output/module-cache")
swiftc "${swift_flags[@]}" -Onone -I "$bridge_output/control" \
    spikes/bridge/sanitizers/Probe.swift "$bridge_output/control/libcontrol.a" \
    -o "$bridge_output/control/probe"
export TSAN_OPTIONS=halt_on_error=1:exitcode=66
bridge_fault=thread
bridge_diagnostic='ThreadSanitizer: data race'
python3 scripts/run-bridge-validation.py --log "$bridge_output/control/clean.log" "$bridge_output/control/probe" clean
for bridge_language in rust swift; do
    if [ "$bridge_language" = rust ]; then bridge_site=eb_sanitizer_thread_fault; else bridge_site=swiftThreadFault; fi
    python3 scripts/run-bridge-validation.py --log "$bridge_output/control/$bridge_language.log" \
        --expect "$bridge_diagnostic" --site "$bridge_site" "$bridge_output/control/probe" "$bridge_language-$bridge_fault"
done

swiftc "${swift_flags[@]}" -O -D BRIDGE_VALIDATION -Xcc -DECHODE_BRIDGE_FAULT_INJECTION \
    -I "$bridge_output/c" "$bridge_output/c/libbridge.a" \
    spikes/bridge/swift/CBridge.swift spikes/bridge/swift/CProbe.swift spikes/bridge/swift/ContractProbe.swift \
    -o "$bridge_output/c-probe"
python3 scripts/run-bridge-validation.py --log "$bridge_output/c-probe.log" "$bridge_output/c-probe"
bash scripts/check-product-bridge.sh thread
