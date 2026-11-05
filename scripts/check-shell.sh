#!/bin/bash
# Build the product C ABI and run the Swift wrapper under Swift 6 strict concurrency.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build -q --locked --offline -p echode-ffi
header="$(find target/debug/build -path '*echode-ffi-*/out/echode.h' -print0 | xargs -0 ls -t | head -1)"
if [[ -z "$header" ]]; then
    echo "check-shell: cbindgen did not write echode.h" >&2
    exit 1
fi
include=swift/EchodeBridge/CEchodeFfi/include
mkdir -p "$include"
cmp -s "$header" "$include/echode.h" || cp "$header" "$include/echode.h"

# One build covers every product, executable and test target; the tests then
# run without a second build plan.
run_swift_tests() {
    local pkg="$1"
    local scratch="$2"
    local log="$scratch/swift-test.log"
    mkdir -p "$scratch"
    if swift build -q --build-tests --package-path "$pkg" --scratch-path "$scratch" \
        -Xswiftc -warnings-as-errors >"$log" 2>&1 &&
        swift test -q --skip-build --package-path "$pkg" --scratch-path "$scratch" \
        -Xswiftc -warnings-as-errors >>"$log" 2>&1; then
        local summary
        summary="$(grep -E 'Test run with [0-9]+ tests? .* passed' "$log" | tail -n 1 || true)"
        echo "$pkg: ${summary:-passed}"
    else
        cat "$log" >&2
        return 1
    fi
}

# RealHALGuard.swift writes this to stderr whenever a test reaches the HAL it replaces,
# also from a thread no test owns. Naming the test needs one test at a time (ADR 0271).
reject_real_hal() {
    local log="target/echode-app-tests/swift-test.log"
    if grep -q 'Real HAL reached:' "$log"; then
        grep 'Real HAL reached:' "$log" >&2
        echo "check-shell: a test reached the real HAL; name it with: make check-real-hal-names" >&2
        return 1
    fi
}

run_swift_tests swift/EchodeBridge target/echode-bridge-tests
if ! run_swift_tests swift/EchodeApp target/echode-app-tests; then
    reject_real_hal || true
    exit 1
fi
reject_real_hal
