#!/bin/bash
# Stop hook: when an agent finishes with uncommitted changes to code or to the checks,
# run `make check`. On failure, ask the agent to fix it before it ends its turn.
# The full output goes to target/agent-check.log.
input=$(cat)

case "$input" in
    *'"status":"completed"'* | *'"status": "completed"'*) ;;
    *)
        echo '{}'
        exit 0
        ;;
esac

cd "$(git rev-parse --show-toplevel 2>/dev/null || pwd)" || {
    echo '{}'
    exit 0
}

changed=$(git status --porcelain -- crates spikes scripts Cargo.toml Cargo.lock Makefile \
    clippy.toml deny.toml rust-toolchain.toml rustfmt.toml 2>/dev/null)
if [ -z "$changed" ]; then
    echo '{}'
    exit 0
fi

mkdir -p target
if make check >target/agent-check.log 2>&1; then
    echo '{}'
else
    echo '{"followup_message": "make check が失敗しました。target/agent-check.log の最後の失敗を読んで原因を直し、make check が通ってから終えてください。既知の不具合のテスト（audit_*）を無視から外した場合は、その項目を直してください。"}'
fi
exit 0
