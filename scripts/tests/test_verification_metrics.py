"""Timing diagnostics must not change evidence or confuse cached work with elapsed time."""
import contextlib
import copy
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import test_verification as support
import test_product_contracts as product_support
import verification_metrics as metrics

verify = support.verify
contracts = product_support.contracts


class TimingTests(unittest.TestCase):
    setUp = support.EvidenceTests.setUp
    git = support.EvidenceTests.git

    def invoke(self):
        with patch.object(verify, "tool_identity", return_value={"rust": "fixture"}), \
             contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            return verify.verify(self.root, support.arguments(only=["check"]))

    report = support.EvidenceTests.report

    def test_history_distinguishes_executed_reused_and_failed_checks(self):
        self.assertEqual(self.invoke(), 0)
        first = self.report()
        self.assertEqual(first["execution"]["status"], "passed")
        self.assertIn("check", first["execution"]["checks"])
        self.assertEqual(first["execution"]["reused"], [])
        self.assertGreaterEqual(first["execution"]["seconds"], first["results"]["check"]["seconds"])
        with patch.object(verify, "resource_snapshot", return_value={"load_average": [99, 99, 99]}):
            self.assertEqual(self.invoke(), 0)
        second = self.report()
        self.assertEqual(second["execution"]["checks"], {})
        self.assertEqual(second["execution"]["reused"], ["check"])
        (self.root / "Makefile").write_text("check:\n\tfalse\n")
        self.assertEqual(self.invoke(), 1)
        last = self.report()
        self.assertEqual(last["execution"]["status"], "failed")
        self.assertEqual(last["execution"]["checks"]["check"]["status"], "failed")
        rows = (self.root / "target/verification/timing-history.jsonl").read_text().splitlines()
        self.assertEqual(len(rows), 3)
        self.assertEqual(json.loads(rows[-1])["execution"]["first_failure"]["id"], "check")
        before = (self.root / "target/verification/report.json").read_bytes()
        text = metrics.render(self.root)
        self.assertIn("not evidence", text)
        self.assertIn("ran=0, reused=1", text)
        self.assertEqual((self.root / "target/verification/report.json").read_bytes(), before)

    def test_legacy_reports_do_not_claim_an_invocation_total(self):
        self.assertEqual(self.invoke(), 0)
        report = self.report()
        report["execution"] = {"order": ["check"]}
        (self.root / "target/verification/report.json").write_text(json.dumps(report))
        self.assertIn("not a measured invocation total", metrics.render(self.root))


class SystemIdentityTests(unittest.TestCase):
    def test_hostname_changes_do_not_expire_evidence_but_os_changes_do(self):
        from types import SimpleNamespace
        def identity(host="local", release="fixture-os", machine="arm64"):
            system = SimpleNamespace(sysname="Darwin", nodename=host, release=release,
                                     version="fixture-kernel", machine=machine)
            with patch.object(verify.os, "uname", return_value=system):
                return verify.tool_identity(support.ROOT)
        # Freeze tool queries; only the system identity differs.
        with patch.object(verify.subprocess, "run", return_value=contracts.subprocess.CompletedProcess([], 0, b"v1", b"")):
            initial = identity()
            self.assertEqual(initial, identity(host="assigned-by-network"))
            self.assertNotEqual(initial, identity(release="new-os"))
            self.assertNotEqual(initial, identity(machine="other-architecture"))


class SwiftBuildTests(unittest.TestCase):
    def test_real_swift_contracts_build_the_test_product_not_apps_or_probes(self):
        policy = verify.load_policy(support.ROOT)
        cases = [case for case in policy["product_contracts"] if case["runner"] == "swift"]
        self.assertTrue(cases)
        for case in cases:
            self.assertEqual(case["build_product"], "EchodeAppTests")
            args = contracts.swift_build(case)
            self.assertNotIn("--build-tests", args)
            self.assertEqual(args[args.index("--product") + 1], case["build_product"])
            self.assertIn("-warnings-as-errors", args)
        jobs = contracts.swift_mutation_jobs(cases)
        self.assertLess(len(jobs), sum(len(contracts.negative_control_mutations(c)) for c in cases))
        self.assertEqual(sum(len(job) for job in jobs),
                         sum(len(contracts.negative_control_mutations(c)) for c in cases))

    def test_interruption_keeps_partial_negative_stage_diagnostics(self):
        case = {"id": "a", "runner": "rust", "package": "p", "target": ["--lib"],
                "test": "a", "timeout": 10, "purpose": "fixture"}
        def good(args, **kwargs):
            return contracts.subprocess.CompletedProcess(args, 0,
                "test a ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;", "")
        def interrupted(root, pending, fingerprints, evidence, record):
            record(contracts.control_result(case, None, True, evidence / "a-negative.log", 1))
            raise KeyboardInterrupt("later stage interrupted")
        with tempfile.TemporaryDirectory() as directory, \
             patch.object(contracts, "negative_controls", side_effect=interrupted):
            root = Path(directory)
            with self.assertRaises(KeyboardInterrupt):
                contracts.run(root, [case], good)
            report = json.loads((root / "target/verification/product-contracts.json").read_text())
            self.assertEqual(report["status"], "running")
            self.assertEqual(report["execution"]["negative_progress"]["completed"], {"rust": 1, "swift": 0})
            self.assertGreaterEqual(report["execution"]["negative_progress"]["seconds"], 0)

    def test_shared_build_still_requires_every_individual_assertion_and_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "View.swift"
            source.write_text("let a = 1\n")
            fault = {"file": "View.swift", "old": "= 1", "new": "= 2"}
            cases = [{"id": name, "runner": "swift", "package_path": "App", "suite": "S",
                      "test": name, "timeout": 10, "negative_control": fault} for name in ("a", "b")]
            builds, tests, results = [], [], []
            def fake(args, **kwargs):
                self.assertIn("= 2", source.read_text())
                if args[1] == "build":
                    builds.append(args)
                    return contracts.subprocess.CompletedProcess(args, 0, "built", "")
                name = args[-1].split("/")[1]
                tests.append((name, kwargs["timeout"]))
                output = (f"Expectation failed\nTest {name}() failed\nTest run with 1 test in 1 suite failed"
                          if name == "a" else f"Test {name}() passed\nTest run with 1 test in 1 suite passed")
                return contracts.subprocess.CompletedProcess(args, 1 if name == "a" else 0, output, "")
            with patch.object(contracts, "run_process", side_effect=fake), \
                 contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                timing = contracts.swift_controls([root], cases, {}, root, results.append)
            self.assertEqual(len(builds), 1)
            self.assertEqual([name for name, _ in tests], ["a", "b"])
            self.assertTrue(all(0 < seconds <= 10 for _, seconds in tests))
            self.assertEqual([result["status"] for result in results], ["passed", "failed"])
            self.assertEqual((timing["builds"], timing["mutations"]), (1, 2))
            self.assertEqual(source.read_text(), "let a = 1\n")
            changed = copy.deepcopy(cases)
            changed[1]["package_path"] = "Other"
            self.assertEqual(len(contracts.swift_mutation_jobs(changed)), 2)
            changed[1]["package_path"] = "App"
            changed[1]["build_product"] = "OtherTests"
            self.assertEqual(len(contracts.swift_mutation_jobs(changed)), 2)
