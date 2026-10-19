#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/build-platform.env
tsan_output="$PWD/target/bridge/thread/errno"
mkdir -p "$tsan_output"
tsan_fixed="$(python3 scripts/tsan_runtime.py --directory fixed)/libclang_rt.tsan_osx_dynamic.dylib"
tsan_baseline="$(python3 scripts/tsan_runtime.py --directory baseline)/libclang_rt.tsan_osx_dynamic.dylib"
xcrun clang -mcpu=apple-m4 -mmacosx-version-min=27.0 -g -O1 -fsanitize=thread \
    -Wall -Wextra -Werror spikes/bridge/sanitizers/Errno.c -o "$tsan_output/probe"
export TSAN_OPTIONS=halt_on_error=1:exitcode=66
# The exact same probe and bootstrap fix run against both runtime variants.
python3 scripts/run-bridge-validation.py --log "$tsan_output/baseline.log" \
    --tsan-runtime "$tsan_baseline" --expect 'ThreadSanitizer: data race' --site reuse_errno \
    "$tsan_output/probe" reuse "$tsan_baseline"
python3 scripts/run-bridge-validation.py --log "$tsan_output/fixed.log" \
    --tsan-runtime "$tsan_fixed" "$tsan_output/probe" reuse "$tsan_fixed"
# Resetting at OS thread birth must not hide a race on a live thread's errno.
python3 scripts/run-bridge-validation.py --log "$tsan_output/real-race.log" \
    --tsan-runtime "$tsan_fixed" --expect 'ThreadSanitizer: data race' --site foreign_errno_write \
    "$tsan_output/probe" race "$tsan_fixed"
