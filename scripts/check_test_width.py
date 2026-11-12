#!/usr/bin/env python3
"""Fail when a Swift test run was not as wide as its check needs (ADR 0210).

The width comes from an experimental variable of Swift Testing, so a toolchain
could drop it without a word. The event stream shows what the run really did.
"""

import argparse
import json
import sys
from pathlib import Path


def peak_live_tests(text):
    """Count ordinary test bodies and parameterized cases, not their containers."""
    functions, live, peak = {}, 0, 0
    for line in text.splitlines():
        try:
            record = json.loads(line)
        except ValueError:
            continue
        payload = record.get("payload", {})
        if record.get("kind") == "test" and payload.get("kind") == "function":
            functions[payload.get("id")] = payload.get("isParameterized", False)
        elif record.get("kind") == "event" and payload.get("testID") in functions:
            # A parameterized function starts before its cases acquire execution
            # slots and ends after all of them finish. Counting that container
            # inflates the width, while counting it once misses parallel cases.
            parameterized = functions[payload["testID"]]
            started = "testCaseStarted" if parameterized else "testStarted"
            ended = "testCaseEnded" if parameterized else "testEnded"
            if payload.get("kind") == started:
                live += 1
                peak = max(peak, live)
            elif payload.get("kind") == ended:
                live -= 1
    return peak


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("events", type=Path)
    limit = parser.add_mutually_exclusive_group(required=True)
    limit.add_argument("--at-most", type=int)
    limit.add_argument("--more-than", type=int)
    options = parser.parse_args(argv)
    text = options.events.read_text(errors="replace") if options.events.exists() else ""
    peak = peak_live_tests(text)
    if not peak:
        print(f"check_test_width: no test ran according to {options.events}", file=sys.stderr)
        return 1
    if options.at_most is not None and peak > options.at_most:
        print(
            f"check_test_width: {peak} tests ran at once, above {options.at_most}. "
            f"Swift Testing may have dropped SWT_EXPERIMENTAL_MAXIMUM_PARALLELIZATION_WIDTH; see {options.events}",
            file=sys.stderr,
        )
        return 1
    if options.more_than is not None and peak <= options.more_than:
        print(
            f"check_test_width: at most {peak} tests ran at once, so tests that need more than "
            f"{options.more_than} together went unchecked for races; see {options.events}",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
