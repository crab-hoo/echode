#!/usr/bin/env bash
# Xcode's ASan ABI stamp differs from upstream Rust's. Keep both guards enabled:
# test Rust's full boundary with its runtime, then Swift integration with Apple's.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/bridge-toolchain.env
bridge_root="$PWD"
bridge_output="$bridge_root/target/bridge/address"
bridge_target=aarch64-apple-darwin
mkdir -p "$bridge_output/c" "$bridge_output/control"
swift --version > "$bridge_output/versions.txt"
rustc "+$BRIDGE_NIGHTLY" -vV >> "$bridge_output/versions.txt"
export ASAN_OPTIONS=halt_on_error=1:detect_leaks=0:detect_stack_use_after_return=1

export CARGO_TARGET_DIR="$bridge_output/rust"
export RUSTFLAGS='-Zsanitizer=address -Cdebuginfo=2 -Cforce-frame-pointers=yes'
python3 scripts/run-bridge-validation.py --timeout 600 --log "$bridge_output/rust-tests.log" \
    cargo "+$BRIDGE_NIGHTLY" test --locked --offline -Zbuild-std --target "$bridge_target" \
    --release --lib -p echode-bridge-spike --features fault-injection
rustc "+$BRIDGE_NIGHTLY" --edition=2024 --crate-type bin \
    --target "$bridge_target" -Zsanitizer=address -Cdebuginfo=2 \
    spikes/bridge/sanitizers/Main.rs -o "$bridge_output/control/rust-probe"
python3 scripts/run-bridge-validation.py --log "$bridge_output/control/rust-clean.log" "$bridge_output/control/rust-probe"
python3 scripts/run-bridge-validation.py --log "$bridge_output/control/rust.log" \
    --expect 'AddressSanitizer: heap-use-after-free' --site eb_sanitizer_memory_fault \
    "$bridge_output/control/rust-probe" rust-memory

# This process instruments Swift only. The same Rust ABI is checked above with
# Rust callbacks; it is not evidence of simultaneous ASan coverage in one process.
unset RUSTFLAGS
export CARGO_TARGET_DIR="$bridge_root/target"
cargo rustc --locked --offline --release --lib --crate-type staticlib \
    -p echode-bridge-spike --features bindgen,fault-injection
cargo build --locked --offline --release --bin echode-bridge-bindgen \
    -p echode-bridge-spike --features bindgen,fault-injection
"$bridge_root/target/release/echode-bridge-bindgen" c-header "$bridge_output/c/echode_bridge.h"
"$bridge_root/target/release/echode-bridge-bindgen" sanitizer-header "$bridge_output/control/probe.h"
cp spikes/bridge/c/module.modulemap "$bridge_output/c/module.modulemap"
cat > "$bridge_output/control/module.modulemap" <<'MODULE'
module CBridgeSanitizerProbe { header "probe.h" export * }
MODULE
rustc "+$BRIDGE_NIGHTLY" --edition=2024 --crate-type staticlib --target "$bridge_target" \
    spikes/bridge/sanitizers/probe.rs -o "$bridge_output/control/libcontrol.a"
swift_flags=(-swift-version 6 -strict-concurrency=complete -warnings-as-errors \
    -target arm64-apple-macos27.0 -parse-as-library -g -sanitize=address \
    -module-cache-path "$bridge_output/module-cache")
swiftc "${swift_flags[@]}" -Onone -I "$bridge_output/control" \
    spikes/bridge/sanitizers/Probe.swift "$bridge_output/control/libcontrol.a" \
    -o "$bridge_output/control/probe"
python3 scripts/run-bridge-validation.py --log "$bridge_output/control/swift-clean.log" "$bridge_output/control/probe" clean
python3 scripts/run-bridge-validation.py --log "$bridge_output/control/swift.log" \
    --expect 'AddressSanitizer: heap-use-after-free' --site swiftMemoryFault \
    "$bridge_output/control/probe" swift-memory
swiftc "${swift_flags[@]}" -O -D BRIDGE_VALIDATION -Xcc -DECHODE_BRIDGE_FAULT_INJECTION \
    -I "$bridge_output/c" "$bridge_root/target/release/libechode_bridge_spike.a" \
    spikes/bridge/swift/CBridge.swift spikes/bridge/swift/CProbe.swift spikes/bridge/swift/ContractProbe.swift \
    -o "$bridge_output/c-probe"
python3 scripts/run-bridge-validation.py --log "$bridge_output/c-probe.log" "$bridge_output/c-probe"
