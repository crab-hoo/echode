#!/usr/bin/env bash
# Interpret only the Rust ownership boundary; Swift integration runs separately.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/bridge-toolchain.env
export CARGO_TARGET_DIR="$PWD/target/bridge/miri"
mkdir -p "$CARGO_TARGET_DIR"
rustc "+$BRIDGE_NIGHTLY" -vV > "$CARGO_TARGET_DIR/versions.txt"
for bridge_seed in 1 17 42; do
    MIRIFLAGS="-Zmiri-seed=$bridge_seed" python3 scripts/run-bridge-validation.py \
        --timeout 600 --log "$CARGO_TARGET_DIR/seed-$bridge_seed.log" \
        cargo "+$BRIDGE_NIGHTLY" miri test --locked --offline \
        -p echode-bridge-spike --lib c_api::tests
done
