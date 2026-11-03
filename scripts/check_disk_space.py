#!/usr/bin/env python3
"""Reject a verification run the disk cannot hold before a long build fills it (XCA-09)."""

import argparse
from pathlib import Path
import shutil
import sys

ROOT = Path(__file__).resolve().parent.parent
GIB = 1 << 30


def cold_estimate(value):
    path, separator, size = value.partition("=")
    if not separator or not path or Path(path).is_absolute() or ".." in Path(path).parts:
        raise argparse.ArgumentTypeError("--cold takes a repository-relative PATH=GIB")
    try:
        gib = float(size)
    except ValueError:
        raise argparse.ArgumentTypeError("--cold takes a repository-relative PATH=GIB") from None
    if not gib > 0:
        raise argparse.ArgumentTypeError("--cold needs a positive size")
    return path, gib


def required_gib(root, minimum, cold):
    """A warm cache grows little per run; a missing one is rebuilt in full."""
    missing = [(path, gib) for path, gib in cold if not (root / path).exists()]
    return minimum + sum(gib for _, gib in missing), [path for path, _ in missing]


def main(argv=None, root=ROOT):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--min-gib", type=float, required=True, help="free space a warm run needs")
    parser.add_argument("--cold", type=cold_estimate, action="append", default=[], metavar="PATH=GIB",
                        help="extra space when the build cache PATH is absent")
    args = parser.parse_args(argv)
    need, missing = required_gib(root, args.min_gib, args.cold)
    free = shutil.disk_usage(root).free / GIB
    cold = f"; cold caches: {', '.join(missing)}" if missing else ""
    if free < need:
        print(f"Free disk space {free:.1f} GiB is below the {need:.1f} GiB this run needs{cold}.", file=sys.stderr)
        return 1
    print(f"Free disk space {free:.1f} GiB covers the {need:.1f} GiB this run needs{cold}.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
