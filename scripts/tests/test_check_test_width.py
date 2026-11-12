"""The width of a Swift run comes from its event stream, not from what was asked."""

import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import check_test_width as check

RECORDS = [
    {"kind": "test", "payload": {"id": "Suite", "kind": "suite"}},
    {"kind": "test", "payload": {"id": "Suite/a", "kind": "function"}},
    {"kind": "test", "payload": {"id": "Suite/b", "kind": "function"}},
    {"kind": "event", "payload": {"kind": "testStarted", "testID": "Suite"}},
    {"kind": "event", "payload": {"kind": "testStarted", "testID": "Suite/a"}},
    {"kind": "event", "payload": {"kind": "testStarted", "testID": "Suite/b"}},
    {"kind": "event", "payload": {"kind": "testEnded", "testID": "Suite/a"}},
    {"kind": "event", "payload": {"kind": "testStarted", "testID": "Suite/a"}},
    {"kind": "event", "payload": {"kind": "testEnded", "testID": "Suite/a"}},
    {"kind": "event", "payload": {"kind": "testEnded", "testID": "Suite/b"}},
]
STREAM = "\n".join(["not json", *map(json.dumps, RECORDS)])


class WidthTests(unittest.TestCase):
    def test_the_peak_counts_test_functions_and_not_suites(self):
        self.assertEqual(check.peak_live_tests(STREAM), 2)
        self.assertEqual(check.peak_live_tests(""), 0)

    def test_parameterized_function_containers_do_not_consume_execution_slots(self):
        records = [
            {"kind": "test", "payload": {"id": "a", "kind": "function"}},
            {"kind": "test", "payload": {"id": "p", "kind": "function", "isParameterized": True}},
            {"kind": "test", "payload": {"id": "q", "kind": "function", "isParameterized": True}},
        ]
        for kind, test_id in [
            ("testStarted", "a"),
            ("testStarted", "p"),
            ("testStarted", "q"),
            ("testCaseStarted", "p"),
            ("testCaseEnded", "p"),
            ("testEnded", "p"),
            ("testEnded", "q"),
            ("testEnded", "a"),
        ]:
            records.append({"kind": "event", "payload": {"kind": kind, "testID": test_id}})
        stream = "\n".join(map(json.dumps, records))
        self.assertEqual(check.peak_live_tests(stream), 2, "parameterized containers inflated the width")
        self.assertEqual(self.verdict(stream, "--at-most", "2")[0], 0)

    def test_concurrent_cases_of_one_parameterized_function_count_separately(self):
        records = [{"kind": "test", "payload": {"id": "p", "kind": "function", "isParameterized": True}}]
        for kind in [
            "testStarted",
            "testCaseStarted",
            "testCaseStarted",
            "testCaseEnded",
            "testCaseEnded",
            "testEnded",
        ]:
            records.append({"kind": "event", "payload": {"kind": kind, "testID": "p"}})
        stream = "\n".join(map(json.dumps, records))
        self.assertEqual(check.peak_live_tests(stream), 2, "concurrent parameter cases escaped the width check")
        self.assertEqual(self.verdict(stream, "--at-most", "1")[0], 1)
        self.assertEqual(self.verdict(stream, "--more-than", "1")[0], 0)

    def verdict(self, text, *limit):
        with tempfile.TemporaryDirectory() as directory:
            events = Path(directory) / "run.events"
            if text is not None:
                events.write_text(text)
            errors = io.StringIO()
            with contextlib.redirect_stderr(errors):
                return check.main([str(events), *limit]), errors.getvalue()

    def test_each_limit_rejects_the_run_that_breaks_it(self):
        self.assertEqual(self.verdict(STREAM, "--at-most", "2")[0], 0)
        self.assertEqual(self.verdict(STREAM, "--more-than", "1")[0], 0)
        code, message = self.verdict(STREAM, "--at-most", "1")
        self.assertEqual(code, 1)
        self.assertIn("2 tests ran at once, above 1", message)
        code, message = self.verdict(STREAM, "--more-than", "2")
        self.assertEqual(code, 1)
        self.assertIn("at most 2 tests ran at once", message)

    def test_a_run_without_events_proves_nothing(self):
        for text in (None, "", "not json"):
            code, message = self.verdict(text, "--at-most", "16")
            self.assertEqual(code, 1)
            self.assertIn("no test ran", message)

    def test_every_pass_or_fail_run_uses_the_one_shared_width(self):
        # ADR 0332: check-swift judges at the sanitizer runs' width and checks it held.
        width = (ROOT / "scripts/swift-test-width.env").read_text()
        self.assertRegex(width, r"(?m)^ECHODE_SWIFT_TEST_WIDTH=16$")
        shell = (ROOT / "scripts/check-shell.sh").read_text()
        bridge = (ROOT / "scripts/check-product-bridge.sh").read_text()
        for script in (shell, bridge):
            self.assertIn("\nsource scripts/swift-test-width.env\n", script)
            self.assertIn("\nunset SWT_EXPERIMENTAL_MAXIMUM_PARALLELIZATION_WIDTH\n", script)
            self.assertNotRegex(script, r"(?m)^\s*(product_)?width=\d")
        self.assertIn('SWT_EXPERIMENTAL_MAXIMUM_PARALLELIZATION_WIDTH="$ECHODE_SWIFT_TEST_WIDTH"', shell)
        self.assertIn(
            'check_test_width.py "$scratch/swift-test.events" \\\n            --at-most "$ECHODE_SWIFT_TEST_WIDTH"',
            shell,
        )
        self.assertIn('product_width="$ECHODE_SWIFT_TEST_WIDTH"', bridge)
        checks = {item["id"]: item for item in json.loads((ROOT / "scripts/verification.json").read_text())["checks"]}
        for name in ("check-swift", "check-bridge-asan", "check-bridge-tsan"):
            with self.subTest(check=name):
                self.assertIn("scripts/swift-test-width.env", checks[name]["inputs"])
                self.assertIn("scripts/check_test_width.py", checks[name]["inputs"])


if __name__ == "__main__":
    unittest.main()
