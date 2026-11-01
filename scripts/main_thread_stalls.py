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

from check_test_width import peak_live_tests

ROOT = Path(__file__).resolve().parents[1]
SOURCE = Path(__file__).with_suffix(".c")
SAMPLE_SECONDS = 0.25
PROFILE_SECONDS = 0.05
WINDOW_SECONDS = 5.0  # The deadline of the Swift tests that poll on the main actor.
EARLY_SECONDS = 8.0  # All tests are alive at once in this first part of an uncapped run.
THREAD_BLOCKED = (3, 4)  # TH_STATE_WAITING and TH_STATE_UNINTERRUPTIBLE.
RUN_TIMEOUT_SECONDS = 300
TEST_NAME = re.compile(r"\b([A-Z]\w*Tests)\.(\w+)")


@dataclass
class Stall:
    end: float
    seconds: float
    samples: list = field(default_factory=list)
    load: dict = field(default_factory=dict)

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
            load = dict(item.split("=", 1) for item in parts[3].split()) if len(parts) == 4 else {}
            stalls.append(Stall(float(parts[0]), float(parts[2]), load={key: float(value) for key, value in load.items()}))
            sample = None
        elif kind == "SAMPLE" and stalls:
            sample = []
            stalls[-1].samples.append(sample)
        elif kind == "frame" and sample is not None and len(parts) == 4:
            sample.append((parts[2], parts[3]))
        else:
            sample = None
    return start_wall, stalls


def parse_profile(text):
    """The continuous main-thread samples of a profile run: (seconds, thread state, frames)."""
    samples, frames = [], None
    for line in text.splitlines():
        parts = line.split(None, 3)
        if len(parts) >= 4 and parts[1] == "PROFILE":
            frames = []
            samples.append((float(parts[2]), int(parts[3]), frames))
        elif len(parts) == 4 and parts[1] == "frame" and frames is not None:
            frames.append((parts[2], parts[3]))
        else:
            frames = None
    return samples


def profile_category(frames):
    """What a main-thread sample was doing, for telling occupation from waiting on the system."""
    images = {image for image, _ in frames}
    symbols = " ".join(symbol for _, symbol in frames)
    if "_doUserDisclosureExpandOrCollapse" in symbols:
        return "disclosure click (nested run loop)"
    if "dyld" in images:
        return "framework load"
    if "libremovefile.dylib" in images:
        return "file removal"
    if "EchodeAppTests" in images:
        return "test code"
    return "no test frame"


def profile_table(runs, start=0.0, end=float("inf")):
    """Main-thread seconds per run by category, with the share the thread spent blocked."""
    counts, blocked = Counter(), Counter()
    for samples in runs:
        for moment, state, frames in samples:
            if start <= moment < end:
                category = profile_category(frames)
                counts[category] += 1
                blocked[category] += state in THREAD_BLOCKED
    return [f"{count * PROFILE_SECONDS / max(len(runs), 1):6.2f} s/run  blocked {100 * blocked[name] // count:3d}%  {name}"
            for name, count in counts.most_common()]


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


def describe_load(stall):
    """Whether a stall was the main thread's work, the machine's saturation, or a starved process."""
    load = stall.load
    if not load:
        return ""
    main = load.get("main_user", 0.0) + load.get("main_system", 0.0)
    return (f"main CPU {main:.2f} s, process {load.get('process', 0.0):.1f} CPU-s, "
            f"runnable {load.get('runnable', 0.0):.1f} s, watcher gap {load.get('watch_gap', 0.0):.2f} s")


def summarize_run(stalls, issues, names):
    lines = [f"longest stall {max((stall.seconds for stall in stalls), default=0.0):.2f} s; "
             f"most silence in {WINDOW_SECONDS:.0f} s {most_silence(stalls):.2f} s"]
    longest = max(stalls, key=lambda stall: stall.seconds, default=None)
    if longest and longest.load:
        lines.append(f"longest stall load: {describe_load(longest)}")
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


def run_once(sanitizer, watcher, directory, number, passthrough, scratch=None, profile=False):
    watch = directory / f"{number}.watch"
    events = directory / f"{number}.events"
    log = directory / f"{number}.log"
    samples = directory / f"{number}.profile"
    for path in (watch, events, samples):
        path.unlink(missing_ok=True)
    environment = os.environ | {"DYLD_INSERT_LIBRARIES": str(watcher), "ECHODE_MAIN_THREAD_STALLS_LOG": str(watch)}
    if profile:
        environment["ECHODE_MAIN_THREAD_PROFILE"] = str(samples)
    if sanitizer == "thread":
        # Same runtime as check-bridge-tsan. xcrun's real path keeps DYLD_* (the /usr/bin shim strips them).
        runtime = subprocess.check_output([sys.executable, "-B", str(ROOT / "scripts/tsan_runtime.py"),
                                           "--directory", "fixed"], text=True).strip()
        environment |= {"DYLD_LIBRARY_PATH": runtime, "TSAN_OPTIONS": "halt_on_error=1:exitcode=66"}
    swift = subprocess.check_output(["xcrun", "--find", "swift"], text=True).strip()
    command = [swift, "test", "-q", "--skip-build", "--package-path", "swift/EchodeApp",
               "--scratch-path", str(scratch or ROOT / f"target/bridge/{sanitizer}/product/swift"),
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
    parser.add_argument("--profile", action="store_true",
                        help="also sample the main thread continuously; suspending it lengthens stalls")
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
    report, runs, profiles = [], [], []
    for number in range(1, options.runs + 1):
        code, stalls, issues = run_once(options.sanitizer, watcher, directory, number, options.swift_test_arguments,
                                        profile=options.profile)
        names = demangled(stalls, xcrun_demangle)
        runs.append((stalls, names))
        events = directory / f"{number}.events"
        peak = peak_live_tests(events.read_text(errors="replace")) if events.exists() else 0
        lines = [f"run {number}: exit {code}, at most {peak} tests at once",
                 *("  " + line for line in summarize_run(stalls, issues, names))]
        print("\n".join(lines), flush=True)
        report += lines
        if options.profile:
            profiles.append(parse_profile((directory / f"{number}.profile").read_text(errors="replace")))
    owners = [f"main-thread silence by owner, about {SAMPLE_SECONDS} s per sample:", *attribution(runs, options.top)]
    if options.profile:
        owners += ["main thread by activity, whole run:", *profile_table(profiles),
                   f"main thread by activity, first {EARLY_SECONDS:.0f} s:", *profile_table(profiles, end=EARLY_SECONDS)]
    print("\n".join(owners))
    # The name matches the bundle pattern of make debug-report.
    (product / "main-thread-stalls.log").write_text("\n".join(report + owners) + "\n")


if __name__ == "__main__":
    main()
