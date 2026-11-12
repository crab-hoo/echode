"""A stalled Swift test run ends with its live tests named, and every run is watched (ADR 0325)."""

import contextlib
import importlib.util
import io
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from pathlib import Path
from unittest.mock import patch

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import swift_test_watchdog as watchdog

FIRST = "EchodeAppTests.FirstTests/first()/FirstTests.swift:8:6"
SECOND = "EchodeAppTests.SecondTests/second()/SecondTests.swift:12:6"

# Writes the given records to the stream, one at a time with a pause, then
# starts a grandchild that outlives a plain kill of this process and waits.
FAKE_RUN = textwrap.dedent(
    """
    import json, subprocess, sys, time
    events, delay, lingering, plan = sys.argv[1], float(sys.argv[2]), sys.argv[3], json.loads(sys.argv[4])
    time.sleep(plan.get("before", 0))
    with open(events, "a") as stream:
        for record in plan["records"]:
            stream.write(json.dumps(record) + "\\n")
            stream.flush()
            time.sleep(delay)
    if plan.get("hang"):
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(600)"])
        with open(lingering, "w") as pid:
            pid.write(str(child.pid))
        time.sleep(600)
    sys.exit(plan.get("exit", 0))
    """
)


def declared(test, parameterized=False):
    return {"kind": "test", "payload": {"id": test, "kind": "function", "isParameterized": parameterized}}


def event(kind, test=None):
    payload = {"kind": kind}
    if test:
        payload["testID"] = test
    return {"kind": "event", "payload": payload}


def alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


class WatchTests(unittest.TestCase):
    def run_fake(self, directory, plan, stall=0.6, delay=0.0):
        """The exit code, stderr, seconds taken and the grandchild's pid file of one watched run."""
        script = directory / "fake.py"
        script.write_text(FAKE_RUN)
        events, lingering = directory / "run.events", directory / "lingering.pid"
        command = [sys.executable, str(script), str(events), str(delay), str(lingering), json.dumps(plan)]
        command += [watchdog.EVENT_PATH, str(events), watchdog.EVENT_VERSION, "0"]
        errors = io.StringIO()
        started = time.monotonic()
        with contextlib.redirect_stderr(errors), patch.object(watchdog, "sample_stacks") as stacks:
            code = watchdog.watch(command, stall=stall, poll=0.05)
        return code, errors.getvalue(), time.monotonic() - started, lingering, stacks

    def test_a_stalled_run_stops_with_its_live_tests_named_and_no_process_left(self):
        with tempfile.TemporaryDirectory() as directory:
            plan = {
                "records": [
                    declared(FIRST),
                    declared(SECOND),
                    event("runStarted"),
                    event("testStarted", FIRST),
                    event("testStarted", SECOND),
                    event("testEnded", FIRST),
                ],
                "hang": True,
            }
            code, errors, seconds, lingering, stacks = self.run_fake(Path(directory), plan)
            self.assertEqual(code, watchdog.STALLED, errors)
            self.assertLess(seconds, 10, "the stall limit, not the fake's own sleep, ended the run")
            self.assertIn("SecondTests/second() (SecondTests.swift:12)", errors)
            self.assertNotIn("FirstTests/first()", errors, "an ended test is not a suspect")
            self.assertIn(str(Path(directory) / "run.events.stall.txt"), errors)
            stacks.assert_called_once()
            grandchild = int(lingering.read_text())
            for _ in range(40):
                if not alive(grandchild):
                    break
                time.sleep(0.05)
            self.assertFalse(alive(grandchild), "a test's own child process outlived the stopped run")

    def test_a_run_that_keeps_writing_is_not_stopped_and_keeps_its_exit_code(self):
        with tempfile.TemporaryDirectory() as directory:
            records = [declared(FIRST), event("runStarted")]
            records += [event("testStarted" if n % 2 else "testEnded", FIRST) for n in range(1, 9)]
            code, errors, seconds, _, stacks = self.run_fake(
                Path(directory), {"records": records, "exit": 3}, stall=0.6, delay=0.25
            )
            self.assertGreater(seconds, 2, "the run outlasted several stall limits in total")
            self.assertEqual(code, 3, errors)
            stacks.assert_not_called()

    def test_the_stall_clock_starts_with_the_first_record_and_ignores_an_old_stream(self):
        # The build before the first record takes as long as it needs, and a
        # stream left by the previous run must not start the clock early.
        with tempfile.TemporaryDirectory() as directory:
            old = Path(directory) / "run.events"
            old.write_text(json.dumps(event("runStarted")) + "\n")
            (Path(directory) / "run.events.stall.txt").write_text("previous run\n")
            plan = {"before": 1.5, "records": [declared(FIRST), event("testStarted", FIRST), event("testEnded", FIRST)]}
            code, errors, _, _, _ = self.run_fake(Path(directory), plan, stall=0.6)
            self.assertEqual(code, 0, errors)
            self.assertFalse((Path(directory) / "run.events.stall.txt").exists())

    def test_a_run_that_does_not_exit_after_its_end_is_stopped_too(self):
        with tempfile.TemporaryDirectory() as directory:
            records = [declared(FIRST), event("testStarted", FIRST), event("testEnded", FIRST), event("runEnded")]
            code, errors, _, _, _ = self.run_fake(Path(directory), {"records": records, "hang": True})
            self.assertEqual(code, watchdog.STALLED, errors)
            self.assertIn("did not exit after the run ended", errors)

    def test_the_live_units_are_parameterized_cases_not_their_containers(self):
        live = watchdog.LiveTests()
        for record in [
            declared(FIRST, parameterized=True),
            event("testStarted", FIRST),
            event("testCaseStarted", FIRST),
            event("testCaseStarted", FIRST),
            event("testCaseEnded", FIRST),
        ]:
            live.feed(record, now=1.0)
        self.assertEqual([test for test, _ in live.live], [FIRST])
        self.assertEqual(live.peak, 2)
        self.assertEqual(watchdog.display(FIRST), "EchodeAppTests.FirstTests/first() (FirstTests.swift:8)")

    def test_the_sample_reaches_the_stack_of_the_stalled_process(self):
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "stall.txt"
            process = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(600)"])
            try:
                watchdog.sample_stacks(process.pid, report)
            finally:
                process.kill()
                process.wait()
            text = report.read_text()
            self.assertIn(f"=== sample of process {process.pid} ===", text)
            self.assertRegex(text, r"(?i)call graph|sample failed")

    def test_a_run_without_a_version_0_stream_is_refused(self):
        errors = io.StringIO()
        with contextlib.redirect_stderr(errors):
            self.assertEqual(watchdog.main(["--", "swift", "test"]), 2)
            self.assertEqual(watchdog.main(["swift", "test"]), 2)
            self.assertEqual(
                watchdog.main(["--", "swift", "test", watchdog.EVENT_PATH, "x", watchdog.EVENT_VERSION, "1"]), 2
            )
        self.assertIn("needs --event-stream-output-path", errors.getvalue())
        self.assertIn("reads --event-stream-version 0 only", errors.getvalue())

    def test_a_signal_to_the_watchdog_stops_the_run_it_started(self):
        with tempfile.TemporaryDirectory() as directory:
            plan = {"records": [declared(FIRST), event("testStarted", FIRST)], "hang": True}
            script = Path(directory) / "fake.py"
            script.write_text(FAKE_RUN)
            events, lingering = Path(directory) / "run.events", Path(directory) / "lingering.pid"
            command = [sys.executable, str(script), str(events), "0", str(lingering), json.dumps(plan)]
            command += [watchdog.EVENT_PATH, str(events), watchdog.EVENT_VERSION, "0"]
            outer = subprocess.Popen(
                [sys.executable, "-B", str(ROOT / "scripts/swift_test_watchdog.py"), "--", *command]
            )
            for _ in range(100):
                if lingering.exists() and lingering.read_text():
                    break
                time.sleep(0.05)
            grandchild = int(lingering.read_text())
            outer.send_signal(signal.SIGTERM)
            self.assertEqual(outer.wait(timeout=10), 128 + signal.SIGTERM)
            for _ in range(40):
                if not alive(grandchild):
                    break
                time.sleep(0.05)
            self.assertFalse(alive(grandchild), "the run outlived the watchdog that started it")


class ContractRunnerTests(unittest.TestCase):
    def test_a_stalled_contract_run_returns_with_the_test_named_instead_of_using_its_timeout(self):
        spec = importlib.util.spec_from_file_location("watched_contracts", ROOT / "scripts/check_product_contracts.py")
        contracts = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(contracts)
        with tempfile.TemporaryDirectory() as directory:
            script = Path(directory) / "fake.py"
            script.write_text(FAKE_RUN)
            plan = {"records": [declared(FIRST), event("runStarted"), event("testStarted", FIRST)], "hang": True}
            command = [sys.executable, str(script), "run.events", "0", "lingering.pid", json.dumps(plan)]
            command += [watchdog.EVENT_PATH, "run.events", watchdog.EVENT_VERSION, "0"]
            started = time.monotonic()
            with (
                patch.object(watchdog, "STALL_SECONDS", 0.6),
                patch.object(watchdog, "sample_stacks"),
                patch.dict(os.environ, {"ECHODE_VERIFY_GROUP": "0"}),
            ):
                completed = contracts.run_process(command, cwd=directory, capture_output=True, text=True, timeout=60)
            self.assertLess(time.monotonic() - started, 30, "the contract's own timeout ended the run")
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("FirstTests/first() (FirstTests.swift:8)", completed.stderr)
            self.assertTrue((Path(directory) / "run.events.stall.txt").exists(), "relative to the run's cwd")


class CoverageTests(unittest.TestCase):
    """Every Swift test run is watched; the watchdog sees Swift Testing only."""

    RUNS = re.compile(r'(?:\bswift|"\$product_swift")\s+test\b(?![^\n]*--show-codecov-path)')

    def logical_lines(self, path):
        return path.read_text().replace("\\\n", " ").splitlines()

    def test_every_swift_test_command_writes_a_stream_and_runs_under_the_watchdog(self):
        sources = [ROOT / "Makefile", *sorted((ROOT / "scripts").glob("*.sh"))]
        found = 0
        for path in sources:
            for line in self.logical_lines(path):
                if not self.RUNS.search(line) or line.lstrip().startswith("#"):
                    continue
                found += 1
                with self.subTest(path=path.name, line=line.strip()[:80]):
                    self.assertIn(watchdog.EVENT_PATH, line)
                    self.assertIn(f"{watchdog.EVENT_VERSION} 0", line)
                    # run-bridge-validation.py watches in-process, keeping DYLD_*.
                    self.assertTrue(
                        "scripts/swift_test_watchdog.py --" in line or "product_validation[@]" in line, line
                    )
        self.assertGreaterEqual(found, 6, "the scan lost the commands it guards")
        validation = (ROOT / "scripts/run-bridge-validation.py").read_text()
        self.assertIn("Watchdog(events) if events else None", validation)
        hal = (ROOT / "scripts/check_real_hal.py").read_text()
        self.assertRegex(hal, r'"scripts/swift_test_watchdog\.py"\),\s*"--",\s*"swift",\s*"test"')
        contracts = (ROOT / "scripts/check_product_contracts.py").read_text()
        self.assertIn('watchdog = Watchdog(Path(cwd or ".") / events) if events else None', contracts)

    def test_swift_test_targets_use_swift_testing_only(self):
        # XCTest writes no event stream, so the watchdog could not see its tests.
        tests = [
            path
            for folder in ("swift", "spikes")
            for path in (ROOT / folder).rglob("*.swift")
            if "/Tests/" in str(path.relative_to(ROOT)) and ".build" not in path.parts
        ]
        self.assertTrue(tests)
        for path in tests:
            with self.subTest(path=str(path.relative_to(ROOT))):
                self.assertNotRegex(path.read_text(), r"(?m)^\s*(@testable\s+)?import\s+XCTest\b")


if __name__ == "__main__":
    unittest.main()
