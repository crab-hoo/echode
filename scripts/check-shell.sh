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
cp "$header" "$include/echode.h"

run_swift_tests() {
    local pkg="$1"
    local scratch="$2"
    local log="$scratch/swift-test.log"
    mkdir -p "$scratch"
    if swift test -q --package-path "$pkg" --scratch-path "$scratch" \
        -Xswiftc -warnings-as-errors >"$log" 2>&1; then
        local summary
        summary="$(grep -E 'Test run with [0-9]+ tests? .* passed' "$log" | tail -n 1 || true)"
        echo "$pkg: ${summary:-passed}"
    else
        cat "$log" >&2
        return 1
    fi
}

run_swift_tests swift/EchodeBridge target/echode-bridge-tests
run_swift_tests swift/EchodeApp target/echode-app-tests
swift build -q --package-path swift/EchodeApp --scratch-path target/echode-app-tests \
    -Xswiftc -warnings-as-errors
