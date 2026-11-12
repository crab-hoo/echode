#!/usr/bin/env python3
"""Wait until an in-progress verification releases its lock, before changing files (ADR 0324).

No agent host runs this as a hook. An agent runs it itself, so the exit code is the answer:
0 once the lock is free, 1 when it is still held at the deadline.
"""

import argparse
import fcntl
import sys
import time
from pathlib import Path

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
LOCK = ROOT / "target/verification/.lock"
# Under the 600-second ceiling of a foreground agent shell; a longer wait is a rerun.
DEFAULT_TIMEOUT = 540


def wait_for_lock(lock_path, timeout):
    """Return once `lock_path` is not exclusively held. A missing file is idle."""
    if not lock_path.exists():
        return True
    deadline = time.monotonic() + timeout
    with lock_path.open("a") as lock:
        while True:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                fcntl.flock(lock, fcntl.LOCK_UN)
                return True
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    return False
                time.sleep(0.05)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT, help="seconds to wait")
    parser.add_argument("--lock", type=Path, default=LOCK, help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    if wait_for_lock(args.lock, args.timeout):
        return 0
    print(
        f"{args.lock} is still held by a running verification after {args.timeout:g} s; "
        "do not change files yet, run this again.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
