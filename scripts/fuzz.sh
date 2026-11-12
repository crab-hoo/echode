#!/usr/bin/env bash
# Builds and runs the libFuzzer targets of crates/echode-fuzz under ASan (ADR 0290).
#   fuzz.sh setup               install the pinned nightly and cargo-fuzz
#   fuzz.sh check               fail unless both are installed
#   fuzz.sh run TARGET SECONDS  fuzz one target
#   fuzz.sh all SECONDS         fuzz every target in turn
# Each run starts from the target's seeds and saved findings. A finding stops the
# run and leaves its input under target/fuzz/artifacts/TARGET/.
set -euo pipefail
cd "$(dirname "$0")/.."
# Shares the validation nightly; this script never edits that file.
source scripts/bridge-toolchain.env
CARGO_FUZZ_VERSION=0.13.2
FUZZ_DIR=crates/echode-fuzz

check() {
    if ! rustc "+$BRIDGE_NIGHTLY" --version >/dev/null 2>&1; then
        echo "toolchain $BRIDGE_NIGHTLY is missing: run make setup-fuzz" >&2
        return 1
    fi
    local installed
    installed="$(cargo fuzz --version 2>/dev/null || true)"
    if [[ "$installed" != "cargo-fuzz $CARGO_FUZZ_VERSION" ]]; then
        echo "cargo-fuzz $CARGO_FUZZ_VERSION is missing (found: ${installed:-none}): run make setup-fuzz" >&2
        return 1
    fi
}

fuzz() {
    cargo "+$BRIDGE_NIGHTLY" fuzz "$1" --fuzz-dir "$FUZZ_DIR" --features libfuzzer "${@:2}"
}

# The [[bin]] entries of the fuzz crate's Cargo.toml.
targets() {
    cargo "+$BRIDGE_NIGHTLY" fuzz list --fuzz-dir "$FUZZ_DIR"
}

run_target() {
    local target="$1" seconds="$2"
    local corpus="target/fuzz/corpus/$target" artifacts="target/fuzz/artifacts/$target"
    local scratch="$PWD/target/fuzz/scratch"
    if ! targets | grep -qx "$target"; then
        echo "unknown fuzz target $target (one of: $(targets | tr '\n' ' '))" >&2
        return 2
    fi
    mkdir -p "$corpus" "$artifacts" "$scratch"
    cargo run -q --locked -p echode-fuzz --example seeds -- "$target" "$corpus"
    # The first directory collects new inputs; the saved findings are only read.
    ECHODE_FUZZ_SCRATCH="$scratch" fuzz run "$target" "$corpus" "$FUZZ_DIR/regressions/$target" -- \
        -max_total_time="$seconds" -timeout=10 -rss_limit_mb=2048 \
        -artifact_prefix="$artifacts/" -print_final_stats=1
    rm -rf "$scratch"
}

case "${1:-}" in
    setup)
        rustup toolchain install "$BRIDGE_NIGHTLY" --profile minimal
        # cargo install reads no project .cargo/config.toml, so keep its release strip off for macOS 27 (ADR 0113).
        CARGO_PROFILE_RELEASE_STRIP=none cargo install --locked "cargo-fuzz@$CARGO_FUZZ_VERSION"
        ;;
    check)
        check
        ;;
    run)
        [[ $# -eq 3 ]] || {
            echo "usage: $0 run TARGET SECONDS" >&2
            exit 2
        }
        check
        fuzz build "$2"
        run_target "$2" "$3"
        ;;
    all)
        [[ $# -eq 2 ]] || {
            echo "usage: $0 all SECONDS" >&2
            exit 2
        }
        check
        fuzz build
        for target in $(targets); do
            run_target "$target" "$2"
        done
        ;;
    *)
        echo "usage: $0 setup | check | run TARGET SECONDS | all SECONDS" >&2
        exit 2
        ;;
esac
