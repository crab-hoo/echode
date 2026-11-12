#!/usr/bin/env bash
# Measures which product lines the Rust and Swift tests run (ADR 0290). Not a gate.
#   coverage.sh setup   install llvm-tools and the pinned cargo-llvm-cov
#   coverage.sh check   fail unless both are installed
#   coverage.sh run     run the tests with coverage and write target/coverage/summary.md
set -euo pipefail
cd "$(dirname "$0")/.."
CARGO_LLVM_COV_VERSION=0.9.1
OUT=target/coverage

check() {
    if ! rustup component list --installed | grep -q '^llvm-tools'; then
        echo "llvm-tools is missing: run make setup-coverage" >&2
        return 1
    fi
    local installed
    installed="$(cargo llvm-cov --version 2>/dev/null || true)"
    if [[ "$installed" != "cargo-llvm-cov $CARGO_LLVM_COV_VERSION" ]]; then
        echo "cargo-llvm-cov $CARGO_LLVM_COV_VERSION is missing (found: ${installed:-none}): run make setup-coverage" >&2
        return 1
    fi
}

# check-base's build and `cargo test --workspace`, instrumented in their own
# target directory so the normal debug build stays warm.
rust_coverage() (
    export CARGO_TARGET_DIR="$PWD/target/llvm-cov-target"
    eval "$(cargo llvm-cov show-env --sh 2>/dev/null)"
    cargo llvm-cov clean --workspace
    {
        bash scripts/build-bundled-plugins.sh &&
            cargo build -q --locked --workspace &&
            cargo test -q --locked --workspace
    } >"$OUT/rust.log" 2>&1 || {
        echo "coverage: Rust tests failed, see $OUT/rust.log" >&2
        exit 1
    }
    cargo llvm-cov report --json --summary-only --output-path "$OUT/rust.json"
)

swift_coverage() {
    local package="$1" name="$2"
    local scratch="$OUT/swift-$name"
    # The test build itself must be instrumented. Numbers from failing tests
    # would mislead, so a failure stops the measurement, as does a stall (ADR 0325).
    python3 -B scripts/swift_test_watchdog.py -- swift test -q --enable-code-coverage --package-path "$package" \
        --scratch-path "$scratch" --event-stream-output-path "$OUT/swift-$name.events" --event-stream-version 0 \
        >"$OUT/swift-$name.log" 2>&1 || {
        echo "coverage: $package tests failed, see $OUT/swift-$name.log" >&2
        return 1
    }
    cp "$(swift test --show-codecov-path --enable-code-coverage --package-path "$package" --scratch-path "$scratch")" \
        "$OUT/swift-$name.json"
}

case "${1:-}" in
    setup)
        rustup component add llvm-tools
        cargo install --locked "cargo-llvm-cov@$CARGO_LLVM_COV_VERSION"
        ;;
    check)
        check
        ;;
    run)
        check
        mkdir -p "$OUT"
        rust_coverage
        # The Swift tests link the uninstrumented Rust library from target/debug,
        # which `make coverage` builds first (contract-prerequisites).
        swift_coverage swift/EchodeBridge bridge
        swift_coverage swift/EchodeApp app
        python3 -B scripts/coverage_summary.py "$OUT" "$OUT/rust.json" "$OUT/swift-bridge.json" "$OUT/swift-app.json"
        ;;
    *)
        echo "usage: $0 setup | check | run" >&2
        exit 2
        ;;
esac
