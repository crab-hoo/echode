#!/bin/bash
# All check selection, caching, and evidence lives in the shared runner.
set -euo pipefail
cd "$(dirname "$0")/../.."
export PYTHONDONTWRITEBYTECODE=1
exec python3 scripts/check_on_stop.py
