#!/bin/bash
# Fails when a Rust file declares foreign functions (an `extern` block with any ABI or
# none, its brace on the same or the next line, or `#[link]`) and is not listed in
# scripts/ffi-allowlist.txt, or when a listed file no longer does.
# The list may only shrink. See docs/reviews/2026-10-08-audit.md, ENG-15, and ENV-05.
set -euo pipefail
cd "$(dirname "$0")/.."

allowlist=scripts/ffi-allowlist.txt
pattern='^[[:space:]]*(unsafe[[:space:]]+)?extern([[:space:]]+"[^"]*")?[[:space:]]*(\{|$)|#\[link\('

declaring=$(grep -rlE --include='*.rs' "$pattern" crates | sort -u || true)
listed=$(grep -vE '^[[:space:]]*(#|$)' "$allowlist" | awk '{print $1}' | sort -u)

new=$(comm -23 <(printf '%s\n' "$declaring") <(printf '%s\n' "$listed") | sed '/^$/d')
stale=$(comm -13 <(printf '%s\n' "$declaring") <(printf '%s\n' "$listed") | sed '/^$/d')

status=0
if [ -n "$new" ]; then
    echo "check-ffi: these files declare foreign functions by hand:"
    printf '  %s\n' $new
    echo "Use libc or the objc2 crates instead (docs/reviews/2026-10-08-audit.md ENG-15)."
    status=1
fi
if [ -n "$stale" ]; then
    echo "check-ffi: these files no longer declare foreign functions; remove them from $allowlist:"
    printf '  %s\n' $stale
    status=1
fi
exit $status
