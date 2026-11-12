#!/usr/bin/env python3
"""Keep the EchodeApp test bundle off the real HAL (ADR 0209).

RealHALGuard.swift replaces CoreAudio's HAL functions inside the test bundle, so
no device opens and any run that reaches one fails. By default this checks the
binding and that the guard stops its control by name; check-shell.sh fails
check-swift when its own run of the tests reaches the HAL (ADR 0271).
A worker thread cannot tell which test it serves, so `--name-tests` runs one test
at a time and takes the name from the event stream.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

from checked_commands import run_logged

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "target/real-hal"
SCRATCH = ROOT / "target/echode-app-tests"
PACKAGE = ["--package-path", "swift/EchodeApp", "--scratch-path", str(SCRATCH), "-Xswiftc", "-warnings-as-errors"]
RUN_TIMEOUT_SECONDS = 300
CONTROL_ENVIRONMENT = "ECHODE_REAL_HAL_CONTROL"
CONTROL_SUITE = "RealHALGuardTests"
MARKER = "Real HAL reached: "
# CoreAudio's clock conversions. They touch no device.
NOT_HAL = {"_AudioConvertHostTimeToNanos", "_AudioGetCurrentHostTime"}
# Every product that plays must keep the real functions.
PRODUCTS = ("Echode", "EchodeAppProbe", "EchodePlayheadProbe")
REQUIRED_IN_PRODUCTS = {"_AudioObjectGetPropertyData", "_AudioDeviceCreateIOProcID", "_AudioDeviceStart"}


def binding_faults(bundle, products):
    """What the linker bound wrongly, from the undefined CoreAudio symbols of each binary."""
    faults = []
    if bundle:
        faults.append(f"the test bundle binds real HAL: {', '.join(sorted(bundle))}. Add them to RealHALGuard.swift.")
    for name, symbols in products.items():
        missing = REQUIRED_IN_PRODUCTS - symbols
        if missing:
            faults.append(
                f"{name} lost the real HAL: {', '.join(sorted(missing))}. RealHALGuard.swift is for the test bundle only."
            )
    return faults


def coreaudio_references(binary):
    listing = subprocess.check_output(["nm", "-u", str(binary)], text=True)
    return {symbol for symbol in listing.split() if symbol.startswith("_Audio")} - NOT_HAL


def display(test):
    """`EchodeAppTests.Suite/name()/File.swift:8:6` as `Suite/name() (File.swift:8)`."""
    name, location = test.removeprefix("EchodeAppTests.").rsplit("/", 1)
    return f"{name} ({location.rsplit(':', 1)[0]})"


def refusals(events, log):
    """(test running, test before it, refusal) in order. The log also holds any refusal after the run."""
    functions, found = set(), []
    running, previous = None, None
    for line in events.splitlines():
        record = json.loads(line)
        payload = record.get("payload", {})
        if record.get("kind") == "test":
            if payload.get("kind") == "function":
                functions.add(payload["id"])
        elif record.get("kind") == "event":
            kind, test = payload.get("kind"), payload.get("testID")
            if kind == "testStarted" and test in functions:
                running = test
            elif kind == "testEnded" and test in functions:
                running, previous = None, test
            elif kind == "issueRecorded":
                for message in payload.get("messages", []):
                    if message.get("text", "").startswith(MARKER):
                        found.append((running, previous, message["text"].removeprefix(MARKER)))
    printed = re.findall("^" + re.escape(MARKER) + "(.*)$", log, re.MULTILINE)
    return found + [(None, previous, text) for text in printed[len(found) :]]


def report(found):
    lines, seen = (
        ["Tests reached the real HAL. Inject sendDevice, sendAudio or sendAudition into their ProjectController:"],
        set(),
    )
    for running, previous, refusal in found:
        call = refusal.split(". ", 1)[0]
        test = running or previous
        if (test, call) in seen:
            continue
        seen.add((test, call))
        if running:
            lines.append(f"  {display(running)}: {call}")
            if previous:
                lines.append(f"    or a request left running by the test before it: {display(previous)}")
        elif previous:
            lines.append(f"  after {display(previous)} ended: {call}")
        else:
            lines.append(f"  before the first test: {call}")
    return lines


def run_tests(name, *arguments, environment=None):
    """The exit code and refusals of one `swift test`; the bundle is already built."""
    events, log = OUTPUT / f"{name}.events", OUTPUT / f"{name}.log"
    command = [
        sys.executable,
        "-B",
        str(ROOT / "scripts/swift_test_watchdog.py"),
        "--",
        "swift",
        "test",
        "--skip-build",
        *PACKAGE,
        "--event-stream-output-path",
        str(events),
        "--event-stream-version",
        "0",
        *arguments,
    ]
    result = run_logged(ROOT, command, log, RUN_TIMEOUT_SECONDS, env=environment)
    text = events.read_text(errors="replace") if events.exists() else ""
    return result["exit_code"], refusals(text, log.read_text(errors="replace"))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--name-tests",
        action="store_true",
        help="also run every test one at a time and name each that reaches the real HAL",
    )
    arguments = parser.parse_args(argv)
    shutil.rmtree(OUTPUT, ignore_errors=True)
    OUTPUT.mkdir(parents=True)
    subprocess.run(["swift", "build", "-q", "--build-tests", *PACKAGE], cwd=ROOT, check=True)
    binaries = Path(
        subprocess.check_output(["swift", "build", "--show-bin-path", *PACKAGE], cwd=ROOT, text=True).strip()
    )
    bundle = binaries / "EchodeAppTests.xctest/Contents/MacOS/EchodeAppTests"
    # Checked first: the control below plays, and only this makes that safe.
    faults = binding_faults(
        coreaudio_references(bundle), {name: coreaudio_references(binaries / name) for name in PRODUCTS}
    )
    if faults:
        print("\n".join(faults), file=sys.stderr)
        return 1

    code, found = run_tests("control", "--filter", CONTROL_SUITE, environment=os.environ | {CONTROL_ENVIRONMENT: "1"})
    if code == 0 or not any(running and CONTROL_SUITE in running for running, _, _ in found):
        print(
            f"The guard did not stop {CONTROL_SUITE} by name (exit {code}); see {OUTPUT / 'control.log'}",
            file=sys.stderr,
        )
        return 1
    if not arguments.name_tests:
        print(
            "check-real-hal: only the test bundle replaces the HAL, and the guard stops by name;"
            " check-swift fails when its run reaches the HAL"
        )
        return 0

    environment = {name: value for name, value in os.environ.items() if name != CONTROL_ENVIRONMENT}
    code, found = run_tests("all", "--no-parallel", environment=environment)
    if found:
        print("\n".join(report(found)), file=sys.stderr)
        print(f"Log: {OUTPUT / 'all.log'}", file=sys.stderr)
        return 1
    if code != 0:
        print(
            f"No test reached the real HAL, but the run failed (exit {code}); see {OUTPUT / 'all.log'}", file=sys.stderr
        )
        return 1
    print("check-real-hal: no test reached the real HAL")
    return 0


if __name__ == "__main__":
    sys.exit(main())
