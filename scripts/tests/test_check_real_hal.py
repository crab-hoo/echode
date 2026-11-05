"""check-real-hal names the test from the event stream and rejects wrong bindings."""

import json
import re
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import check_real_hal as check  # noqa: E402
import verify  # noqa: E402

FIRST = "EchodeAppTests.FirstTests/first()/FirstTests.swift:8:6"
SECOND = "EchodeAppTests.SecondTests/second()/SecondTests.swift:12:6"
SUITE = "EchodeAppTests.SecondTests"
REFUSAL = "AudioObjectGetPropertyData on echode-worker. Inject sendDevice or sendAudio."


def declared(test, kind="function"):
    return json.dumps({"kind": "test", "payload": {"id": test, "kind": kind}})


def event(kind, test=None, *messages):
    payload = {"kind": kind, "messages": [{"symbol": "details", "text": text} for text in messages]}
    if test:
        payload["testID"] = test
    return json.dumps({"kind": "event", "payload": payload})


def stream(*events):
    return "\n".join([declared(FIRST), declared(SECOND), declared(SUITE, "suite"), *events])


class RefusalTests(unittest.TestCase):
    def test_a_refusal_belongs_to_the_function_running_not_its_suite(self):
        text = stream(event("testStarted", FIRST), event("testEnded", FIRST), event("testStarted", SUITE),
                      event("testStarted", SECOND), event("issueRecorded", None, "Issue recorded", check.MARKER + REFUSAL),
                      event("testEnded", SECOND), event("testEnded", SUITE))
        self.assertEqual(check.refusals(text, check.MARKER + REFUSAL + "\n"), [(SECOND, FIRST, REFUSAL)])

    def test_other_issues_are_not_refusals(self):
        text = stream(event("testStarted", FIRST), event("issueRecorded", FIRST, "Expectation failed"), event("testEnded", FIRST))
        self.assertEqual(check.refusals(text, "↳ " + check.MARKER + "quoted by the test output\n"), [])

    def test_a_refusal_only_in_the_log_came_after_the_run(self):
        # Swift Testing drops an issue once the run has ended; stderr still has it.
        text = stream(event("testStarted", FIRST), event("testEnded", FIRST))
        self.assertEqual(check.refusals(text, check.MARKER + REFUSAL + "\n"), [(None, FIRST, REFUSAL)])

    def test_the_report_names_each_test_once_with_the_test_before_it(self):
        lines = check.report([(SECOND, FIRST, REFUSAL), (SECOND, FIRST, REFUSAL), (None, SECOND, REFUSAL)])
        self.assertEqual(lines[1:], ["  SecondTests/second() (SecondTests.swift:12): AudioObjectGetPropertyData on echode-worker",
                                     "    or a request left running by the test before it: FirstTests/first() (FirstTests.swift:8)"])

    def test_the_report_names_a_refusal_between_tests(self):
        self.assertEqual(check.report([(None, FIRST, REFUSAL)])[1:],
                         ["  after FirstTests/first() (FirstTests.swift:8) ended: AudioObjectGetPropertyData on echode-worker"])


class BindingTests(unittest.TestCase):
    def test_a_guarded_bundle_and_real_products_pass(self):
        self.assertEqual(check.binding_faults(set(), {"Echode": set(check.REQUIRED_IN_PRODUCTS) | {"_AudioDeviceStop"}}), [])

    def test_a_real_function_left_in_the_bundle_fails(self):
        faults = check.binding_faults({"_AudioDeviceStart"}, {})
        self.assertEqual(len(faults), 1)
        self.assertIn("_AudioDeviceStart", faults[0])

    def test_a_product_without_the_real_functions_fails(self):
        faults = check.binding_faults(set(), {"Echode": {"_AudioDeviceStart"}})
        self.assertEqual(len(faults), 1)
        self.assertIn("Echode lost the real HAL", faults[0])

    def test_clock_conversions_are_not_hal(self):
        listing = "_AudioGetCurrentHostTime\n_AudioConvertHostTimeToNanos\n_AudioDeviceStop\n_free\n"
        with patch.object(check.subprocess, "check_output", return_value=listing):
            self.assertEqual(check.coreaudio_references("binary"), {"_AudioDeviceStop"})


class SelectionTests(unittest.TestCase):
    def test_the_check_covers_every_side_of_the_path_to_hal(self):
        policy = verify.load_policy(ROOT)
        entry = next(item for item in policy["checks"] if item["id"] == "check-real-hal")
        self.assertEqual(entry["mode"], "automatic")
        self.assertIn("check-real-hal", verify.select_checks(policy, ["crates/echode-audio-io/src/hardware.rs"]))
        self.assertLessEqual({"swift/EchodeApp/**", "swift/EchodeBridge/**", "scripts/check_real_hal.py"}, set(entry["inputs"]))
        # The Rust side of the path comes through the FFI library's dependencies.
        folders = {folder.name for folder in map(Path, verify.package_directories(ROOT, entry["packages"]))}
        self.assertLessEqual({"echode-ffi", "echode-document", "echode-audio-io"}, folders)

    def test_check_swift_scans_its_own_run_for_the_guard_marker(self):
        # ADR 0271: the parallel check-swift run is the gate; naming the test is opt-in.
        guard = (ROOT / "swift/EchodeApp/Tests/EchodeAppTests/RealHALGuard.swift").read_text()
        shell = (ROOT / "scripts/check-shell.sh").read_text()
        marker = re.search(r'private let marker = "([^"]+)"', guard)[1]
        self.assertEqual(check.MARKER.strip(), marker)
        self.assertIn(f"grep -q '{marker}' \"$log\"", shell)
        self.assertIn("make check-real-hal-names", shell)
        self.assertRegex((ROOT / "Makefile").read_text(),
                         r"(?m)^check-real-hal-names:\n\tpython3 -B scripts/check_real_hal\.py --name-tests$")
        policy = verify.load_policy(ROOT)
        swift = next(item for item in policy["checks"] if item["id"] == "check-swift")
        self.assertIn("scripts/check-shell.sh", swift["inputs"])

    def test_the_makefile_runs_the_check(self):
        self.assertRegex((ROOT / "Makefile").read_text(), r"(?m)^check-real-hal:\n(\t.*\n)*\tpython3 -B scripts/check_real_hal\.py$")


if __name__ == "__main__":
    unittest.main()
