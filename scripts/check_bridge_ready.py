#!/usr/bin/env python3
"""Read-only checks for the pinned validation compiler, rust-src, and Miri driver."""

import argparse
from pathlib import Path
import os
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def check(root, miri=False):
    text = (root / "scripts/bridge-toolchain.env").read_text()
    match = re.search(r"^BRIDGE_NIGHTLY=([a-z0-9-]+)$", text, re.MULTILINE)
    if not match:
        raise ValueError("invalid BRIDGE_NIGHTLY")
    toolchain = "+" + match[1]
    env = {**os.environ, "RUSTUP_AUTO_INSTALL": "0"}
    result = subprocess.run(["rustc", toolchain, "--print", "sysroot"], cwd=root, env=env,
                            capture_output=True, text=True, check=True, timeout=10)
    source = Path(result.stdout.strip()) / "lib/rustlib/src/rust/library/Cargo.toml"
    if not source.is_file():
        raise ValueError("pinned nightly rust-src is missing")
    if miri:
        subprocess.run(["cargo", toolchain, "miri", "--version"], cwd=root, env=env,
                       capture_output=True, text=True, check=True, timeout=10)

# Readiness does not prepare Miri's sysroot or fetch build-std dependencies.
# Those remain setup-bridge responsibilities; the real checks retain their guards.

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--miri", action="store_true")
    args = parser.parse_args()
    try:
        check(ROOT, args.miri)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"Bridge prerequisites: {error}; run make setup-bridge", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
