"""A green process exit must also prove the required regression ran."""

import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("contracts", ROOT / "scripts/check_product_contracts.py")
contracts = importlib.util.module_from_spec(spec)
spec.loader.exec_module(contracts)


class ContractEvidenceTests(unittest.TestCase):
    def test_child_processes_stay_in_the_outer_verification_group(self):
        process = Mock(returncode=0)
        process.communicate.return_value = ("", "")
        with patch.dict(contracts.os.environ, {"ECHODE_VERIFY_GROUP": "1"}), \
             patch.object(contracts.os, "getsid", return_value=42), \
             patch.object(contracts.os, "getpgrp", return_value=42), \
             patch.object(contracts.subprocess, "Popen", return_value=process) as popen:
            contracts.run_process(["test"], cwd=ROOT, capture_output=True, text=True, timeout=1)
            self.assertFalse(popen.call_args.kwargs["start_new_session"])

    def test_a_direct_run_owns_and_stops_its_child_group(self):
        process = Mock(pid=1234)
        process.communicate.side_effect = [subprocess.TimeoutExpired(["test"], 1), ("", "")]
        with patch.dict(contracts.os.environ, {"ECHODE_VERIFY_GROUP": ""}), \
             patch.object(contracts.subprocess, "Popen", return_value=process) as popen, \
             patch.object(contracts.os, "killpg") as killpg:
            with self.assertRaises(subprocess.TimeoutExpired):
                contracts.run_process(["test"], cwd=ROOT, capture_output=True, text=True, timeout=1)
            self.assertTrue(popen.call_args.kwargs["start_new_session"])
            killpg.assert_called_once_with(1234, contracts.signal.SIGKILL)

    def test_rust_requires_one_unignored_named_test(self):
        case = {"runner": "rust", "test": "path::contract"}
        good = "test path::contract ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured;"
        self.assertTrue(contracts.passed(case, 0, good))
        for output in ("test result: ok. 0 passed; 0 failed; 1 ignored;", "running 0 tests",
                       good.replace("path::contract", "path::other"), good.replace("... ok", "... ignored")):
            self.assertFalse(contracts.passed(case, 0, output))
        self.assertFalse(contracts.passed(case, 1, good))

    def test_swift_does_not_accept_empty_xctest_or_disabled_tests(self):
        case = {"runner": "swift", "test": "contractHeader"}
        good = "✔ Test contractHeader() passed after 0.01 seconds.\n✔ Test run with 1 test passed after 0.02 seconds."
        self.assertTrue(contracts.passed(case, 0, good))
        for output in ("Executed 0 tests, with 0 failures", "✔ Test run with 0 tests passed",
                       "◇ Test contractHeader() skipped", good.replace("contractHeader", "differentTest"),
                       good + "\nknown issue"):
            self.assertFalse(contracts.passed(case, 0, output))

    def test_missing_test_stops_the_gate_and_preserves_evidence(self):
        case = {"id": "required", "runner": "rust", "package": "p", "target": ["--lib"],
                "test": "contract", "timeout": 1, "purpose": "test the gate"}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def empty_run(args, **kwargs):
                return subprocess.CompletedProcess(args, 0, "running 0 tests", "")
            self.assertEqual(contracts.run(root, [case], empty_run), 1)
            self.assertTrue((root / "target/verification/product-contracts.json").is_file())

    def test_an_empty_inventory_is_not_a_success(self):
        with self.assertRaises(ValueError):
            contracts.run(ROOT, [])

    def test_negative_controls_must_fail_in_the_test_body(self):
        rust = {"runner": "rust", "test": "contract"}
        swift = {"runner": "swift", "test": "contract"}
        self.assertTrue(contracts.rejected_behavior(rust, 101,
            "test contract ... FAILED\ntest result: FAILED. 0 passed; 1 failed; 0 ignored;"))
        self.assertTrue(contracts.rejected_behavior(swift, 1,
            "Expectation failed\nTest contract() failed\nTest run with 1 test in 1 suite failed"))
        for case in (rust, swift):
            self.assertFalse(contracts.rejected_behavior(case, 1, "compiler error"))
            self.assertFalse(contracts.rejected_behavior(case, 1, "missing compiler"))
            self.assertFalse(contracts.rejected_behavior(case, 0, "passed"))

    def test_mutations_cannot_follow_a_symlink_outside_the_copy(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            checkout = root / "copy"
            checkout.mkdir()
            outside = root / "original.rs"
            outside.write_text("original")
            (checkout / "escape.rs").symlink_to(outside)
            for filename in ("../original.rs", "escape.rs", str(outside)):
                with self.assertRaises(ValueError):
                    contracts.mutation_path(checkout, filename)
            self.assertEqual(outside.read_text(), "original")
