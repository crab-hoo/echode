#!/bin/bash
# Exercise the actual package, queue, snapshots and Swift controller. ASan is
# Swift-only here (Rust has separate ASan tests); TSan instruments both sides.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/bridge-toolchain.env
product_sanitizer="${1:?usage: check-product-bridge.sh address|thread}"
product_output="$PWD/target/bridge/$product_sanitizer/product"
product_validation=(python3 scripts/run-bridge-validation.py --timeout 600)
product_swift=swift
product_swift_flags=(-Xswiftc -warnings-as-errors)
# The width that decides pass or fail, shared with check-swift (ADR 0210, ADR 0332).
source scripts/swift-test-width.env
product_width="$ECHODE_SWIFT_TEST_WIDTH"
unset SWT_EXPERIMENTAL_MAXIMUM_PARALLELIZATION_WIDTH
mkdir -p "$product_output"
case "$product_sanitizer" in
    address)
        unset RUSTFLAGS
        export CARGO_TARGET_DIR="$PWD/target"
        # The default members: the library the app ships (ADR 0328).
        cargo build --locked --offline
        export ECHODE_RUST_LIBRARY_DIR="$PWD/target/debug"
        ;;
    thread)
        product_runtime="$(python3 scripts/tsan_runtime.py --directory fixed)/libclang_rt.tsan_osx_dynamic.dylib"
        product_validation+=(--tsan-runtime "$product_runtime")
        # The /usr/bin Swift forwarding shim strips DYLD_* before forwarding.
        product_swift="$(xcrun --find swift)"
        # Swift has no condition for the sanitizer. ContinuationHandoff.swift needs this.
        product_swift_flags+=(-Xswiftc -DECHODE_TSAN)
        # Same nightly, target, profile and flags as the TSan probe: reuse std/dependencies.
        export CARGO_TARGET_DIR="$PWD/target/bridge/thread/rust"
        export RUSTFLAGS='-Ctarget-cpu=apple-m4 -Zsanitizer=thread -Zexternal-clangrt -Cdebuginfo=2 -Cforce-frame-pointers=yes'
        cargo "+$BRIDGE_NIGHTLY" rustc --locked --offline -Zbuild-std --target aarch64-apple-darwin \
            --release --lib --crate-type staticlib -p echode-ffi
        export ECHODE_RUST_LIBRARY_DIR="$CARGO_TARGET_DIR/aarch64-apple-darwin/release"
        ;;
    *) exit 2 ;;
esac
product_header="$(
    python3 - "$CARGO_TARGET_DIR" <<'PY'
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
cmp -s "$product_header" swift/EchodeBridge/CEchodeFfi/include/echode.h ||
    cp "$product_header" swift/EchodeBridge/CEchodeFfi/include/echode.h
# Keep every test and the recovery suite's 5s readiness deadline. A separate
# process keeps unrelated concurrent fixtures out of the recovery readiness check.
# The validation runner stops a run whose event stream stalls (ADR 0325).
SWT_EXPERIMENTAL_MAXIMUM_PARALLELIZATION_WIDTH="$product_width" \
    "${product_validation[@]}" --log "$product_output/swift-tests.log" "$product_swift" test -q \
    --package-path swift/EchodeApp --scratch-path "$product_output/swift" \
    --sanitize "$product_sanitizer" "${product_swift_flags[@]}" --skip RecoveryConsentTests \
    --event-stream-output-path "$product_output/swift-tests.events" --event-stream-version 0
python3 scripts/check_test_width.py "$product_output/swift-tests.events" --at-most "$product_width"
"${product_validation[@]}" --log "$product_output/recovery-swift-tests.log" "$product_swift" test -q --skip-build \
    --package-path swift/EchodeApp --scratch-path "$product_output/swift" \
    --sanitize "$product_sanitizer" "${product_swift_flags[@]}" --filter RecoveryConsentTests \
    --event-stream-output-path "$product_output/recovery-swift-tests.events" --event-stream-version 0
if ! grep -Eq 'Test run with [1-9][0-9]* tests? .*passed' "$product_output/recovery-swift-tests.log"; then
    echo "product bridge: recovery suite must execute at least one test" >&2
    exit 1
fi
if [ "$product_sanitizer" = thread ]; then
    # The width above hides races between tests that never overlap (ADR 0210), so all of
    # them run together once more. The run above judged the tests; this one finds reports.
    "${product_validation[@]}" --races-only --log "$product_output/race-swift-tests.log" "$product_swift" test -q --skip-build \
        --package-path swift/EchodeApp --scratch-path "$product_output/swift" \
        --sanitize "$product_sanitizer" "${product_swift_flags[@]}" --skip RecoveryConsentTests \
        --event-stream-output-path "$product_output/race-swift-tests.events" --event-stream-version 0
    python3 scripts/check_test_width.py "$product_output/race-swift-tests.events" --more-than "$product_width"
fi
