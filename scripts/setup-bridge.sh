#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/bridge-toolchain.env
rustup toolchain install "$BRIDGE_NIGHTLY" --profile minimal --component rust-src --component miri
cargo fetch --locked
# Preparing Miri also fetches the fixed nightly std dependencies used by build-std.
cargo "+$BRIDGE_NIGHTLY" miri setup
