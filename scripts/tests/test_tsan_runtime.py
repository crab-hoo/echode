"""Validation must use the pinned runtime and reject stale or mixed runtimes."""

import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("tsan_runtime", ROOT / "scripts/tsan_runtime.py")
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


FAKE_SWIFT = """\
import json, os, sys
arguments = sys.argv[1:]
environment = os.environ
width = environment.get("SWT_EXPERIMENTAL_MAXIMUM_PARALLELIZATION_WIDTH", "")
with open(environment["ECHODE_TEST_COMMAND_LOG"], "a") as log:
    log.write("\\n".join(["__SWIFT_CALL__", *arguments, "WIDTH=" + width]) + "\\n")
if "--sanitize" in arguments and arguments[arguments.index("--sanitize") + 1] == "thread":
    print("dyld[1]: <fixture> " + LIBRARY)
recovery = "--filter" in arguments
wide = "--skip-build" in arguments and not recovery
if recovery and environment.get("ECHODE_TEST_FAIL_RECOVERY") == "1":
    sys.exit(7)
if "--event-stream-output-path" in arguments:
    alive = 16 if environment.get("ECHODE_TEST_WIDE_NARROW") == "1" and wide else 40
    if width and environment.get("ECHODE_TEST_IGNORE_WIDTH") != "1":
        alive = int(width)
    records = [{"kind": "test", "payload": {"id": str(n), "kind": "function"}} for n in range(alive)]
    records += [{"kind": "event", "payload": {"kind": "testStarted", "testID": str(n)}} for n in range(alive)]
    with open(arguments[arguments.index("--event-stream-output-path") + 1], "w") as events:
        events.write("\\n".join(map(json.dumps, records)) + "\\n")
if wide and environment.get("ECHODE_TEST_WIDE_REPORT") == "1":
    print("WARNING: ThreadSanitizer: data race (pid=1)\\nSUMMARY: ThreadSanitizer: data race Fixture.swift:1 in fixture")
    sys.exit(66)
if wide and environment.get("ECHODE_TEST_WIDE_CRASH") == "1":
    sys.exit(139)
failed = wide and environment.get("ECHODE_TEST_WIDE_FAILS") == "1"
if failed or (not wide and not recovery and environment.get("ECHODE_TEST_FAIL_TESTS") == "1"):
    print("Test run with 40 tests in 1 suite failed after 0.001 seconds with 1 issue.")
    sys.exit(1)
print("Test run with " + environment.get("ECHODE_TEST_COUNT", "2") + " tests in 1 suite passed after 0.001 seconds.")
"""


class RuntimeTests(unittest.TestCase):
    def product_fixture(self, root, sanitizer):
        """A copy of the product check whose tools only record what they were asked."""
        scripts = root / "scripts"
        scripts.mkdir()
        for name in (
            "check-product-bridge.sh",
            "run-bridge-validation.py",
            "check_test_width.py",
            "bridge-toolchain.env",
            "build-platform.env",
        ):
            shutil.copy2(ROOT / "scripts" / name, scripts / name)
        target = "target/debug" if sanitizer == "address" else "target/bridge/thread/rust/aarch64-apple-darwin/release"
        header = root / target / "build/echode-ffi-fixture/out/echode.h"
        header.parent.mkdir(parents=True)
        header.write_text("/* fixture header */")
        (root / "swift/EchodeBridge/CEchodeFfi/include").mkdir(parents=True)
        tools = root / "tools"
        tools.mkdir()
        (tools / "cargo").write_text("#!/bin/sh\nexit 0\n")
        library = root / "runtime" / runtime.LIBRARY
        library.parent.mkdir()
        library.write_bytes(b"loader protocol fixture, never loaded")
        (scripts / "tsan_runtime.py").write_text(f"print({str(library.parent)!r})\n")
        (tools / "xcrun").write_text(f'#!/bin/sh\necho "{tools / "swift"}"\n')
        (tools / "swift").write_text(f"#!{sys.executable}\n" + FAKE_SWIFT.replace("LIBRARY", repr(str(library))))
        for tool in tools.iterdir():
            tool.chmod(0o755)
        return scripts, tools, root / "swift-arguments.log"

    def product_check(self, root, sanitizer, **environment):
        scripts, tools, log = root / "scripts", root / "tools", root / "swift-arguments.log"
        return subprocess.run(
            ["/bin/bash", str(scripts / "check-product-bridge.sh"), sanitizer],
            capture_output=True,
            text=True,
            timeout=20,
            env={
                **os.environ,
                "PATH": str(tools) + ":" + os.environ["PATH"],
                "ECHODE_TEST_COMMAND_LOG": str(log),
                **environment,
            },
            check=False,
        )

    def test_address_check_still_launches_swift_with_macos_bash(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _, _, log = self.product_fixture(root, "address")
            result = self.product_check(root, "address")
            self.assertEqual(result.returncode, 0, result.stderr)
            calls = [call.splitlines() for call in log.read_text().split("__SWIFT_CALL__\n")[1:]]
            self.assertEqual(len(calls), 2)
            for arguments in calls:
                self.assertEqual(arguments[0], "test")
                self.assertEqual(arguments[arguments.index("--sanitize") + 1], "address")
            self.assertEqual(calls[0][calls[0].index("--skip") + 1], "RecoveryConsentTests")
            self.assertEqual(calls[1][calls[1].index("--filter") + 1], "RecoveryConsentTests")
            self.assertIn("--skip-build", calls[1])
            self.assertEqual(calls[0][-1], "WIDTH=16", "the tests are judged at the capped width")
            failed = self.product_check(root, "address", ECHODE_TEST_FAIL_RECOVERY="1")
            self.assertNotEqual(failed.returncode, 0)
            self.assertIn("recovery-swift-tests.failed.log", failed.stderr)
            empty = self.product_check(root, "address", ECHODE_TEST_COUNT="0")
            self.assertNotEqual(empty.returncode, 0)
            self.assertIn("recovery suite must execute at least one test", empty.stderr)
            ignored = self.product_check(root, "address", ECHODE_TEST_IGNORE_WIDTH="1")
            self.assertNotEqual(ignored.returncode, 0, "a dropped variable must not bring all the tests back silently")
            self.assertIn("40 tests ran at once, above 16", ignored.stderr)
            judged = self.product_check(root, "address", ECHODE_TEST_FAIL_TESTS="1")
            self.assertNotEqual(judged.returncode, 0, "the capped run judges its tests")

    def test_thread_check_judges_tests_narrow_and_looks_for_races_wide(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _, _, log = self.product_fixture(root, "thread")
            # A variable left in the environment must not narrow the wide run.
            result = self.product_check(root, "thread", SWT_EXPERIMENTAL_MAXIMUM_PARALLELIZATION_WIDTH="4")
            self.assertEqual(result.returncode, 0, result.stderr)
            calls = [call.splitlines() for call in log.read_text().split("__SWIFT_CALL__\n")[1:]]
            self.assertEqual(len(calls), 3)
            self.assertEqual(calls[0][-1], "WIDTH=16")
            self.assertEqual(calls[1][calls[1].index("--filter") + 1], "RecoveryConsentTests")
            self.assertEqual(calls[2][-1], "WIDTH=")
            self.assertEqual(calls[2][calls[2].index("--skip") + 1], "RecoveryConsentTests")
            self.assertIn("--skip-build", calls[2])
            # A deadline may run out in the wide run: the capped run already judged that test.
            timing = self.product_check(root, "thread", ECHODE_TEST_WIDE_FAILS="1")
            self.assertEqual(timing.returncode, 0, timing.stderr)
            self.assertIn("race-swift-tests.failed.log", timing.stdout)
            report = self.product_check(root, "thread", ECHODE_TEST_WIDE_REPORT="1")
            self.assertNotEqual(report.returncode, 0)
            self.assertIn("SUMMARY: ThreadSanitizer: data race", report.stderr)
            crash = self.product_check(root, "thread", ECHODE_TEST_WIDE_CRASH="1")
            self.assertNotEqual(crash.returncode, 0, "a run that did not finish found nothing")
            narrow = self.product_check(root, "thread", ECHODE_TEST_WIDE_NARROW="1")
            self.assertNotEqual(narrow.returncode, 0, "a narrow wide run would hide the races it is for")
            self.assertIn("at most 16 tests ran at once", narrow.stderr)

    def test_both_runtime_artifacts_and_the_build_context_must_match(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(runtime, "CACHE", Path(directory)):
            libraries = {}
            for variant in ("baseline", "fixed"):
                path = runtime.CACHE / variant / runtime.LIBRARY
                path.parent.mkdir()
                path.write_bytes(variant.encode())
                libraries[variant] = runtime.sha256(path)
            manifest = runtime.CACHE / "manifest.json"
            manifest.write_text(json.dumps({"context": {"fixture": 1}, "libraries": libraries}))
            self.assertTrue(runtime.prepared({"fixture": 1}))
            self.assertFalse(runtime.prepared({"fixture": 2}))
            (runtime.CACHE / "baseline" / runtime.LIBRARY).write_bytes(b"changed")
            self.assertFalse(runtime.prepared({"fixture": 1}))
            manifest.write_text("{invalid")
            self.assertFalse(runtime.prepared({"fixture": 1}))

    def test_validation_refuses_missing_or_mixed_loader_provenance(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            library = root / runtime.LIBRARY
            library.write_bytes(b"loader protocol fixture, never loaded")
            for libraries, accepted in (
                ([], False),
                ([library], True),
                ([library, root / "other" / runtime.LIBRARY], False),
            ):
                lines = "\n".join(f"dyld[42]: <fixture> {path}" for path in libraries)
                result = subprocess.run(
                    [
                        sys.executable,
                        str(ROOT / "scripts/run-bridge-validation.py"),
                        "--log",
                        str(root / "probe.log"),
                        "--tsan-runtime",
                        str(library),
                        sys.executable,
                        "-c",
                        f"print({lines!r})",
                    ],
                    capture_output=True,
                    text=True,
                    timeout=10,
                    check=False,
                )
                self.assertEqual(result.returncode == 0, accepted, result.stderr)

    def test_a_failed_run_keeps_its_log_and_shows_the_report_not_loader_lines(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            log = root / "swift-tests.log"
            report = (
                "WARNING: ThreadSanitizer: data race (pid=1)\n  Read of size 8 by thread T8:\n"
                "SUMMARY: ThreadSanitizer: data race Document.swift:59 in fixture\n"
            )
            noise = "dyld[1]: <fixture> /usr/lib/fixture.dylib\n" * 2000
            program = f"import sys; sys.stderr.write({(noise + report + noise)!r}); sys.exit(6)"
            result = subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "scripts/run-bridge-validation.py"),
                    "--log",
                    str(log),
                    sys.executable,
                    "-c",
                    program,
                ],
                capture_output=True,
                text=True,
                timeout=10,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("SUMMARY: ThreadSanitizer: data race Document.swift:59", result.stderr)
            self.assertNotIn("fixture.dylib", result.stderr)
            kept = root / "swift-tests.failed.log"
            self.assertIn(str(kept), result.stderr)
            # A passing rerun rewrites the log but leaves the failing evidence.
            subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "scripts/run-bridge-validation.py"),
                    "--log",
                    str(log),
                    sys.executable,
                    "-c",
                    "print('ok')",
                ],
                check=True,
                timeout=10,
            )
            self.assertEqual(log.read_text(), "ok\n")
            self.assertIn(report, kept.read_text())


if __name__ == "__main__":
    unittest.main()
