#!/bin/bash
# Exercise the actual package, queue, snapshots and Swift controller. ASan is
# Swift-only here (Rust has separate ASan tests); TSan instruments both sides.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/bridge-toolchain.env
product_sanitizer="${1:?usage: check-product-bridge.sh address|thread}"
product_output="$PWD/target/bridge/$product_sanitizer/product"
product_validation=(python3 scripts/run-bridge-validation.py --timeout 600 --log "$product_output/swift-tests.log")
product_swift=swift
mkdir -p "$product_output"
case "$product_sanitizer" in
    address)
        unset RUSTFLAGS
        export CARGO_TARGET_DIR="$PWD/target"
        cargo build --locked --offline -p echode-ffi
        export ECHODE_RUST_LIBRARY_DIR="$PWD/target/debug"
        ;;
    thread)
        product_runtime="$(python3 scripts/tsan_runtime.py --directory fixed)/libclang_rt.tsan_osx_dynamic.dylib"
        product_validation+=(--tsan-runtime "$product_runtime")
        # The /usr/bin Swift forwarding shim strips DYLD_* before forwarding.
        product_swift="$(xcrun --find swift)"
        export CARGO_TARGET_DIR="$product_output/rust"
        export RUSTFLAGS='-Ctarget-cpu=apple-m4 -Zsanitizer=thread -Zexternal-clangrt -Cdebuginfo=2 -Cforce-frame-pointers=yes'
        cargo "+$BRIDGE_NIGHTLY" rustc --locked --offline -Zbuild-std --target aarch64-apple-darwin \
            --release --lib --crate-type staticlib -p echode-ffi
        export ECHODE_RUST_LIBRARY_DIR="$CARGO_TARGET_DIR/aarch64-apple-darwin/release"
        ;;
    *) exit 2 ;;
esac
product_header="$(python3 - "$CARGO_TARGET_DIR" <<'PY'
from pathlib import Path
import sys

root = Path(sys.argv[1])
# Stable uses crate-hash/out; the validation nightly uses crate/hash/out.
headers = [path for path in root.rglob("echode.h") if path.parent.name == "out"
           and any(parent.name == "echode-ffi" or parent.name.startswith("echode-ffi-")
                   for parent in path.parents)]
if not headers:
    raise SystemExit("product bridge: cbindgen did not write echode.h")
print(max(headers, key=lambda path: path.stat().st_mtime_ns))
PY
)"
cp "$product_header" swift/EchodeBridge/CEchodeFfi/include/echode.h
"${product_validation[@]}" "$product_swift" test -q \
    --package-path swift/EchodeApp --scratch-path "$product_output/swift" \
    --sanitize "$product_sanitizer" -Xswiftc -warnings-as-errors
