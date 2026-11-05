#!/bin/bash
# Check each handwritten foreign symbol, including additions to listed modules.
set -euo pipefail
cd "$(dirname "$0")/.."
exec python3 -B scripts/check_ffi.py
