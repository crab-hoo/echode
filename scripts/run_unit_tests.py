#!/usr/bin/env python3
"""Run every scripts/tests module in its own process, several at once (ADR 0271).

The modules share no state: each test builds its fixtures in a temporary directory.
Run one after another they took 90 s; the slowest module now bounds the wall time.
A failed, erroring or empty module fails the run and prints its whole output.
"""

import os
import re
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TESTS = ROOT / "scripts/tests"
# The slowest modules compile probes; more workers than this only add contention.
MAXIMUM_WORKERS = 8


def modules(directory=TESTS):
    return sorted(path.stem for path in directory.glob("test_*.py"))


def run_module(name, directory=TESTS):
    started = time.monotonic()
    result = subprocess.run(
        [sys.executable, "-B", "-m", "unittest", "discover", "-s", str(directory), "-p", name + ".py"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
        check=False,
    )
    return {
        "module": name,
        "code": result.returncode,
        "output": result.stdout + result.stderr,
        "seconds": time.monotonic() - started,
    }


def summary(result):
    """(tests run, tests skipped) from unittest's closing lines, or None when it did not get that far."""
    ran = re.search(r"^Ran ([0-9]+) tests? in ", result["output"], re.MULTILINE)
    if ran is None:
        return None
    skipped = re.search(r"^(?:OK|FAILED) \(.*skipped=([0-9]+)", result["output"], re.MULTILINE)
    return int(ran[1]), int(skipped[1]) if skipped else 0


def main(directory=TESTS):
    names = modules(directory)
    if not names:
        print(f"no test modules in {directory}", file=sys.stderr)
        return 1
    started = time.monotonic()
    with ThreadPoolExecutor(max_workers=max(1, min(MAXIMUM_WORKERS, os.cpu_count() or 1, len(names)))) as pool:
        results = list(pool.map(lambda name: run_module(name, directory), names))
    failed = []
    total = skipped = 0
    for result in results:
        counts = summary(result)
        if result["code"] != 0 or counts is None or counts[0] == 0:
            failed.append(result)
            continue
        total += counts[0]
        skipped += counts[1]
    for result in failed:
        print(f"== {result['module']} (exit {result['code']})", file=sys.stderr)
        print(result["output"], file=sys.stderr)
    elapsed = time.monotonic() - started
    if failed:
        print(
            f"scripts/tests: {len(failed)} of {len(names)} modules failed: "
            + ", ".join(result["module"] for result in failed),
            file=sys.stderr,
        )
        return 1
    print(f"scripts/tests: {total} tests in {len(names)} modules passed (skipped={skipped}) in {elapsed:.1f} s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
