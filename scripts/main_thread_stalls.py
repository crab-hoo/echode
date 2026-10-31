#!/usr/bin/env python3
"""Show what holds the main thread during sanitizer Swift runs (docs/debugging.md).

Report only: stall lengths never decide a pass or a failure. Build first with
make check-bridge-tsan or make check-bridge-asan.
"""

import argparse
from collections import Counter
from dataclasses import dataclass, field
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
SOURCE = Path(__file__).with_suffix(".c")
SAMPLE_SECONDS = 0.25
WINDOW_SECONDS = 5.0  # The deadline of the Swift tests that poll on the main actor.
RUN_TIMEOUT_SECONDS = 300
TEST_NAME = re.compile(r"\b([A-Z]\w*Tests)\.(\w+)")


@dataclass
class Stall:
    end: float
    seconds: float
    samples: list = field(default_factory=list)

    @property
    def start(self):
        return self.end - self.seconds


def parse_watch(text):
    """The wall clock of the process start and its stalls, with sampled frames."""
    start_wall, stalls, sample = 0.0, [], None
    for line in text.splitlines():
        parts = line.split(None, 3)
        if len(parts) < 2:
            continue
        kind = parts[1]
        if kind == "START":
            start_wall = float(parts[2])
        elif kind == "STALL":
            stalls.append(Stall(float(parts[0]), float(parts[2])))
            sample = None
        elif kind == "SAMPLE" and stalls:
            sample = []
            stalls[-1].samples.append(sample)
        elif kind == "frame" and sample is not None and len(parts) == 4:
            sample.append((parts[2], parts[3]))
        else:
            sample = None
    return start_wall, stalls


def issues_from_events(text, start_wall):
    """Recorded test issues, in seconds since the process start."""
    issues = []
    for line in text.splitlines():
        try:
            record = json.loads(line)
        except ValueError:
            continue
        event = record.get("payload", {}) if record.get("kind") == "event" else {}
        if event.get("kind") == "issueRecorded":
            messages = " ".join(message.get("text", "") for message in event.get("messages", []))
            issues.append((event.get("instant", {}).get("since1970", start_wall) - start_wall, messages))
    return issues


def demangled(stalls, demangle):
    names = sorted({symbol for stall in stalls for sample in stall.samples for image, symbol in sample
                    if image == "EchodeAppTests" and symbol.startswith("$s")})
    return dict(zip(names, demangle(names))) if names else {}


def label(sample, names):
    """The test that owns a sample; a test's thunks may not demangle to its name."""
    tag = next((name for image, name in (("dyld", "framework load"), ("libremovefile.dylib", "file removal"))
                if any(frame[0] == image for frame in sample)), None)
    test = None
    for image, symbol in reversed(sample):
        match = TEST_NAME.search(names.get(symbol, "")) if image == "EchodeAppTests" else None
        if match:
            test = f"{match[1]}.{match[2]}"
            break
    return (test or "(no test frame)") + (f" [{tag}]" if tag else "")


def most_silence(stalls, window=WINDOW_SECONDS):
    """The most silence inside any window: what a concurrent deadline of that length can lose."""
    return max((sum(max(0.0, min(other.end, stall.start + window) - max(other.start, stall.start)) for other in stalls)
                for stall in stalls), default=0.0)


def summarize_run(stalls, issues, names):
    lines = [f"longest stall {max((stall.seconds for stall in stalls), default=0.0):.2f} s; "
             f"most silence in {WINDOW_SECONDS:.0f} s {most_silence(stalls):.2f} s"]
    for moment, message in issues:
        lines.append(f"issue at {moment:.3f} s: {message[:140]}")
        for stall in stalls:
            if stall.end > moment - WINDOW_SECONDS - 2 and stall.start < moment:
                owners = sorted({label(sample, names) for sample in stall.samples})
                lines.append(f"  stall {stall.start:.3f}..{stall.end:.3f} ({stall.seconds:.3f} s) {', '.join(owners)}")
    return lines


def attribution(runs, top):
    counts = Counter(label(sample, names) for stalls, names in runs for stall in stalls for sample in stall.samples)
    return [f"{count * SAMPLE_SECONDS / max(len(runs), 1):6.2f} s/run  {name}" for name, count in counts.most_common(top)]


def xcrun_demangle(names):
    return subprocess.run(["xcrun", "swift", "demangle", "--simplified"], input="\n".join(names),
                          capture_output=True, text=True, check=True).stdout.splitlines()


def run_once(sanitizer, watcher, directory, number, passthrough):
    watch = directory / f"{number}.watch"
    events = directory / f"{number}.events"
    log = directory / f"{number}.log"
    for path in (watch, events):
        path.unlink(missing_ok=True)
    environment = os.environ | {"DYLD_INSERT_LIBRARIES": str(watcher), "ECHODE_MAIN_THREAD_STALLS_LOG": str(watch)}
    if sanitizer == "thread":
        # Same runtime as check-bridge-tsan. xcrun's real path keeps DYLD_* (the /usr/bin shim strips them).
        runtime = subprocess.check_output([sys.executable, "-B", str(ROOT / "scripts/tsan_runtime.py"),
                                           "--directory", "fixed"], text=True).strip()
        environment |= {"DYLD_LIBRARY_PATH": runtime, "TSAN_OPTIONS": "halt_on_error=1:exitcode=66"}
    swift = subprocess.check_output(["xcrun", "--find", "swift"], text=True).strip()
    command = [swift, "test", "-q", "--skip-build", "--package-path", "swift/EchodeApp",
               "--scratch-path", str(ROOT / f"target/bridge/{sanitizer}/product/swift"),
               "--sanitize", sanitizer, "-Xswiftc", "-warnings-as-errors",
               "--event-stream-output-path", str(events), "--event-stream-version", "0", *passthrough]
    with log.open("w") as output:
        # Its own process group: killing only the driver would orphan the test helper.
        process = subprocess.Popen(command, cwd=ROOT, env=environment, stdout=output, stderr=subprocess.STDOUT,
                                   start_new_session=True)
        try:
            code = process.wait(timeout=RUN_TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise SystemExit(f"run {number} exceeded {RUN_TIMEOUT_SECONDS} s; see {log}") from None
    if not watch.exists():
        raise SystemExit(f"the watcher did not start in the test process; see {log}")
    start_wall, stalls = parse_watch(watch.read_text(errors="replace"))
    issues = issues_from_events(events.read_text(errors="replace"), start_wall) if events.exists() else []
    return code, stalls, issues


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("sanitizer", choices=["thread", "address"])
    parser.add_argument("--runs", type=int, default=1)
    parser.add_argument("--top", type=int, default=12)
    parser.add_argument("swift_test_arguments", nargs="*", default=["--skip", "RecoveryConsentTests"],
                        help="after --, replaces the default of skipping the separately run recovery suite")
    options = parser.parse_args(argv)
    product = ROOT / f"target/bridge/{options.sanitizer}/product"
    if not (product / "swift").is_dir():
        make = "check-bridge-tsan" if options.sanitizer == "thread" else "check-bridge-asan"
        raise SystemExit(f"no sanitizer build at {product / 'swift'}; run make {make} first")
    directory = product / "main-thread-stalls"
    shutil.rmtree(directory, ignore_errors=True)
    directory.mkdir(parents=True)
    watcher = directory / "libstalls.dylib"
    subprocess.run(["xcrun", "clang", "-arch", "arm64", "-arch", "arm64e", "-dynamiclib", "-O1", "-g", "-Wall", "-Wextra",
                    "-Werror", str(SOURCE), "-o", str(watcher)], check=True)
    report, runs = [], []
    for number in range(1, options.runs + 1):
        code, stalls, issues = run_once(options.sanitizer, watcher, directory, number, options.swift_test_arguments)
        names = demangled(stalls, xcrun_demangle)
        runs.append((stalls, names))
        lines = [f"run {number}: exit {code}", *("  " + line for line in summarize_run(stalls, issues, names))]
        print("\n".join(lines), flush=True)
        report += lines
    owners = [f"main-thread silence by owner, about {SAMPLE_SECONDS} s per sample:", *attribution(runs, options.top)]
    print("\n".join(owners))
    # The name matches the bundle pattern of make debug-report.
    (product / "main-thread-stalls.log").write_text("\n".join(report + owners) + "\n")


if __name__ == "__main__":
    main()
