#!/bin/bash
# Source-changing tools wait until the stop hook's verification releases its lock.
set -euo pipefail
cd "$(dirname "$0")/../.."
export PYTHONDONTWRITEBYTECODE=1
exec python3 scripts/wait_for_verify.py
