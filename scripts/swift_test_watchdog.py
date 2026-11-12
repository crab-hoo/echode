#!/usr/bin/env python3
"""Stop a Swift test run that stalls and name the tests it was running (ADR 0325).

Swift Testing's `.timeLimit` only cancels a test's task. A test blocked on a
thread, or on an `await` nobody resumes, keeps the whole run alive. This reads
the event stream the run writes and stops the run from outside instead:
once the run has written its first record, a gap of STALL_SECONDS without a
new one ends it. The report names the live tests and keeps a stack sample.

Usage: swift_test_watchdog.py -- COMMAND... --event-stream-output-path PATH --event-stream-version 0
"""

import json
import os
import signal
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

# Healthy runs never go 3.1 s without a record, and no test body has taken more
# than 11 s, both under TSan (ADR 0325). The limit leaves room for a busy machine.
STALL_SECONDS = 120
POLL_SECONDS = 0.25
SAMPLE_SECONDS = 1
STALLED = 124
EVENT_PATH = "--event-stream-output-path"
EVENT_VERSION = "--event-stream-version"
RUNNER = "swiftpm-testing-helper"


class LiveTests:
    """Test bodies and parameterized cases that started and have not ended.

    A parameterized function starts before its cases acquire execution slots
    and ends after all of them finish, so its cases count instead (ADR 0210).
    """

    def __init__(self):
        self.functions = {}
        self.live = []
        self.peak = 0
        self.ended = False

    def feed(self, record, now=0.0):
        payload = record.get("payload", {})
        if record.get("kind") == "test":
            if payload.get("kind") == "function":
                self.functions[payload.get("id")] = payload.get("isParameterized", False)
            return
        if record.get("kind") != "event":
            return
        kind, test = payload.get("kind"), payload.get("testID")
        if kind == "runEnded":
            self.ended = True
        if test not in self.functions:
            return
        parameterized = self.functions[test]
        if kind == ("testCaseStarted" if parameterized else "testStarted"):
            self.live.append((test, now))
            self.peak = max(self.peak, len(self.live))
        elif kind == ("testCaseEnded" if parameterized else "testEnded"):
            for index, (live, _) in enumerate(self.live):
                if live == test:
                    del self.live[index]
                    break


def records(text):
    """The JSON objects of a stream; the toolchain may print other lines into it."""
    for line in text.splitlines():
        try:
            record = json.loads(line)
        except ValueError:
            continue
        if isinstance(record, dict):
            yield record


class EventReader:
    """The complete records a run has appended since the last read."""

    def __init__(self, path):
        self.path, self.offset, self.partial = path, 0, b""

    def read(self):
        try:
            with self.path.open("rb") as stream:
                stream.seek(self.offset)
                data = stream.read()
        except FileNotFoundError:
            return []
        self.offset += len(data)
        *lines, self.partial = (self.partial + data).split(b"\n")
        return list(records(b"\n".join(lines).decode(errors="replace")))


def display(test):
    """`Module.Suite/name()/File.swift:8:6` as `Module.Suite/name() (File.swift:8)`."""
    name, separator, location = test.rpartition("/")
    if not separator or ".swift:" not in location:
        return test
    return f"{name} ({location.rsplit(':', 1)[0]})"


def event_path(command):
    """The stream the command writes, or None. The parser reads version 0 only."""
    if EVENT_PATH not in command[:-1]:
        return None
    if EVENT_VERSION not in command[:-1] or command[command.index(EVENT_VERSION) + 1] != "0":
        raise ValueError(f"the watchdog reads {EVENT_VERSION} 0 only")
    return Path(command[command.index(EVENT_PATH) + 1])


def descendants(pid):
    """Every process below pid with its executable, parents before children."""
    listing = subprocess.run(["ps", "-A", "-o", "pid=,ppid=,comm="], capture_output=True, text=True, check=False)
    children = {}
    for line in listing.stdout.splitlines():
        fields = line.split(None, 2)
        if len(fields) == 3 and fields[0].isdigit() and fields[1].isdigit():
            children.setdefault(int(fields[1]), []).append((int(fields[0]), fields[2]))
    found, pending = [], [pid]
    while pending:
        for child in children.get(pending.pop(0), []):
            found.append(child)
            pending.append(child[0])
    return found


def stop_tree(pid):
    # The run stays in the caller's process group, so the verifier's own
    # timeout still reaches it. Only this run's processes are stopped here.
    for target in [pid, *(child for child, _ in descendants(pid))]:
        try:
            os.kill(target, signal.SIGKILL)
        except ProcessLookupError:
            pass


def sample_stacks(pid, output):
    """Append a stack sample of each test runner below pid, or of pid itself."""
    targets = [child for child, name in descendants(pid) if Path(name).name == RUNNER] or [pid]
    with output.open("a") as report:
        for target in targets:
            with tempfile.TemporaryDirectory() as directory:
                sample = Path(directory) / "sample.txt"
                try:
                    subprocess.run(
                        ["sample", str(target), str(SAMPLE_SECONDS), "-file", str(sample)],
                        capture_output=True,
                        timeout=60,
                        check=False,
                    )
                    text = sample.read_text(errors="replace")
                except (OSError, subprocess.SubprocessError) as error:
                    text = f"sample failed: {error}\n"
            report.write(f"\n=== sample of process {target} ===\n{text}")


def stall_report(live, seconds, now):
    lines = [f"swift_test_watchdog: no Swift Testing record for {seconds} s; stopped the run (ADR 0325)."]
    if live.live:
        lines.append("  Still running, oldest first:")
        lines += [f"    {display(test)}, {now - started:.0f} s" for test, started in live.live]
    elif live.ended:
        lines.append("  No test was running: the process did not exit after the run ended.")
    else:
        lines.append("  No test was running: the run stopped between tests.")
    return lines


class Watchdog:
    """Watch one run's event stream from a thread and stop the run when it stalls.

    Create it before the run starts, so a stream left by an earlier run is gone,
    then `start` with the run's pid and `finish` once the run has exited. Callers
    that set DYLD_* use this in-process: SIP's /usr/bin/python3 would drop them.
    """

    def __init__(self, events, stall=None, poll=None):
        self.events = Path(events)
        self.stacks = self.events.with_name(self.events.name + ".stall.txt")
        self.events.parent.mkdir(parents=True, exist_ok=True)
        for stale in (self.events, self.stacks):
            stale.unlink(missing_ok=True)
        self.stall = STALL_SECONDS if stall is None else stall
        self.poll = POLL_SECONDS if poll is None else poll
        self.report = []
        self._finished = threading.Event()
        self._thread = None

    def start(self, pid):
        self._thread = threading.Thread(target=self._watch, args=(pid,), daemon=True)
        self._thread.start()

    def finish(self):
        """The stall report, empty when the run ended by itself."""
        self._finished.set()
        if self._thread:
            self._thread.join()
        return self.report

    def _watch(self, pid):
        reader, live, last = EventReader(self.events), LiveTests(), None
        while not self._finished.wait(self.poll):
            now = time.monotonic()
            for record in reader.read():
                live.feed(record, now)
                last = now
            if last is None or now - last <= self.stall or self._finished.is_set():
                continue
            lines = stall_report(live, self.stall, now)
            self.stacks.write_text("\n".join(lines) + "\n")
            sample_stacks(pid, self.stacks)
            stop_tree(pid)
            self.report = [*lines, f"  Stacks: {self.stacks}"]
            return


def watch(command, stall=None, poll=None, **options):
    """Run command and return its exit code, or STALLED after stopping it."""
    events = event_path(command)
    if events is None:
        raise ValueError(f"the command needs {EVENT_PATH} PATH {EVENT_VERSION} 0")
    if not events.is_absolute():
        events = Path(options.get("cwd") or ".") / events
    watchdog = Watchdog(events, stall, poll)
    process = subprocess.Popen(command, **options)

    def forward(signum, _frame):
        stop_tree(process.pid)
        sys.exit(128 + signum)

    previous = {number: signal.signal(number, forward) for number in (signal.SIGTERM, signal.SIGHUP, signal.SIGALRM)}
    watchdog.start(process.pid)
    try:
        code = process.wait()
    finally:
        report = watchdog.finish()
        for number, handler in previous.items():
            signal.signal(number, handler)
    if report:
        print("\n".join(report), file=sys.stderr, flush=True)
        return STALLED
    return code


def main(argv=None):
    arguments = list(sys.argv[1:] if argv is None else argv)
    if arguments[:1] != ["--"] or len(arguments) < 2:
        print(__doc__.strip().splitlines()[-1], file=sys.stderr)
        return 2
    try:
        return watch(arguments[1:])
    except ValueError as error:
        print(f"swift_test_watchdog: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
