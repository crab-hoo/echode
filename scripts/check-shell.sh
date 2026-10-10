#!/bin/bash
# Build the product C ABI and run the Swift wrapper under Swift 6 strict concurrency.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --locked --offline -p echode-ffi
header="$(find target/debug/build -path '*echode-ffi-*/out/echode.h' -print0 | xargs -0 ls -t | head -1)"
if [[ -z "$header" ]]; then
    echo "check-shell: cbindgen did not write echode.h" >&2
    exit 1
fi
include=swift/EchodeBridge/CEchodeFfi/include
mkdir -p "$include"
cp "$header" "$include/echode.h"
swift test --package-path swift/EchodeBridge --scratch-path target/echode-bridge-tests \
    -Xswiftc -warnings-as-errors
