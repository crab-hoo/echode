"""Execution safety, selected-only prerequisites, and failure detection order."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import test_verification as support

verify = support.verify
ROOT = support.ROOT


class ExecutionTests(unittest.TestCase):
    setUp = support.EvidenceTests.setUp
    git = support.EvidenceTests.git
    report = support.EvidenceTests.report
    run_verify = support.EvidenceTests.run_verify

    def policy(self, checks, preflights=None):
        policy = json.loads((self.root / verify.POLICY).read_text())
        policy["checks"] = checks
        policy["preflights"] = preflights or {}
        (self.root / verify.POLICY).write_text(json.dumps(policy))
        return policy

    def check(self, name, **extra):
        return {"id": name, "mode": "automatic", "always": True, "areas": [],
                "inputs": ["src/**"], "timeout": 10, "purpose": "fixture", **extra}

    def test_make_recipes_cannot_hide_pipeline_or_intermediate_failures(self):
        for recipe in ("false | cat", "false; echo status=$?", "false; true"):
            with self.subTest(recipe=recipe):
                (self.root / "Makefile").write_text("check:\n\t@" + recipe + "\n")
                result = verify.run_check(self.root, self.check("check"), self.root / "run.log")
                self.assertEqual(result["status"], "failed", "an earlier failure was hidden")

    def test_order_is_stable_topological_and_preserves_the_requested_set(self):
        checks = [self.check("slow"), self.check("asan", priority=30),
                  self.check("tsan", priority=0, after=["runtime"]),
                  self.check("runtime", priority=20), self.check("infra", priority=10)]
        policy = self.policy(checks)
        self.assertEqual([c["id"] for c in verify.execution_order(policy, {c["id"] for c in checks})],
                         ["infra", "runtime", "tsan", "asan", "slow"])
        self.assertEqual([c["id"] for c in verify.execution_order(policy, {"tsan", "slow"})],
                         ["tsan", "slow"])
        for mutation in ("unknown", "cycle", "bad-priority", "unknown-preflight"):
            with self.subTest(mutation=mutation):
                broken = json.loads(json.dumps(checks))
                if mutation == "unknown":
                    broken[0]["after"] = ["absent"]
                elif mutation == "cycle":
                    broken[0]["after"] = ["asan"]
                    broken[1]["after"] = ["slow"]
                elif mutation == "bad-priority":
                    broken[0]["priority"] = "first"
                else:
                    broken[0]["preflight"] = ["absent"]
                self.policy(broken)
                with self.assertRaises(ValueError):
                    verify.load_policy(self.root)

    def test_preflight_aborts_before_any_check_and_records_environment_failure(self):
        self.policy([self.check("check", preflight=["runtime"])], {
            "runtime": {"command": [sys.executable, "-c", "raise SystemExit(7)"],
                        "timeout": 5, "remedy": "make setup-bridge"}})
        self.assertEqual(self.run_verify(support.arguments()), (1, 0))
        report = self.report()
        self.assertEqual(report["results"]["check"]["status"], "pending")
        self.assertEqual(report["preflight"]["runtime"]["exit_code"], 7)
        self.assertIn("make setup-bridge", (self.root / "target/verification/report.md").read_text())
        self.assertEqual(report["execution"]["first_failure"]["kind"], "preflight")
        self.assertGreaterEqual(report["execution"]["first_failure"]["seconds"], 0)

    def test_prerequisites_only_cover_uncached_requested_executions_and_are_deduplicated(self):
        counter = self.root / "target/preflight-count"
        command = [sys.executable, "-c", f"from pathlib import Path; p=Path({str(counter)!r}); "
                   "p.write_text(p.read_text()+'x' if p.exists() else 'x')"]
        checks = [self.check("check", preflight=["ready"]), self.check("other", preflight=["ready"]),
                  self.check("measure", mode="manual", preflight=["broken"])]
        self.policy(checks, {"ready": {"command": command, "timeout": 5, "remedy": "prepare fixture"},
                             "broken": {"command": ["missing-fixture-tool"], "timeout": 5, "remedy": "prepare fixture"}})
        self.assertEqual(self.run_verify(support.arguments(defer=["measure"], reason="no hardware")), (0, 2))
        self.assertEqual(counter.read_text(), "x")
        self.assertEqual(self.run_verify(support.arguments()), (0, 0))
        self.assertEqual(counter.read_text(), "x")
        self.assertEqual(self.run_verify(support.arguments(only=["other"], force=True)), (0, 1))
        self.assertEqual(counter.read_text(), "xx")
        self.assertEqual(self.run_verify(support.arguments(only=["measure"], run=["measure"])), (1, 0))
        self.assertEqual(self.report()["preflight"]["broken"]["status"], "failed")

    def test_failure_latency_uses_the_actual_order_not_the_average_runtime(self):
        self.policy([self.check("slow"), self.check("asan", priority=30), self.check("infra", priority=10)])
        called = []
        def run(root, check, log, force=False, base=None):
            called.append(check["id"])
            return {"status": "failed" if check["id"] == "asan" else "passed", "seconds": 1}
        self.assertEqual(self.run_verify(support.arguments(), run), (1, 2))
        self.assertEqual(called, ["infra", "asan"])
        execution = self.report()["execution"]
        self.assertEqual(execution["order"], ["infra", "asan", "slow"])
        self.assertEqual(execution["first_failure"]["id"], "asan")
        self.assertGreaterEqual(execution["first_failure"]["seconds"], 0)

    def test_preflight_definitions_and_source_inputs_expire_the_dependent_evidence(self):
        script = self.root / "scripts/ready.py"
        script.write_text("print('ready')\n")
        self.policy([self.check("check", preflight=["ready"])], {
            "ready": {"command": [sys.executable, "scripts/ready.py"], "timeout": 5, "remedy": "prepare fixture"}})
        self.assertEqual(self.run_verify(support.arguments()), (0, 1))
        self.assertEqual(self.run_verify(support.arguments()), (0, 0))
        script.write_text("raise SystemExit(9)\n")
        self.assertEqual(self.run_verify(support.arguments()), (1, 0))
        self.assertEqual(self.report()["results"]["check"]["status"], "pending")
        policy = verify.load_policy(self.root)
        self.assertIn("check", verify.select_checks(policy, ["scripts/ready.py"]))
        before = verify.current_definitions(self.root, policy)
        policy["preflights"]["ready"]["timeout"] += 1
        self.assertNotEqual(before, verify.current_definitions(self.root, policy))

    def test_real_policy_keeps_cheap_gates_and_asan_ahead_of_product_contracts(self):
        policy = verify.load_policy(ROOT)
        order = [c["id"] for c in verify.execution_order(policy, {c["id"] for c in policy["checks"]})]
        self.assertLess(order.index("check-format"), order.index("check-verification"))
        self.assertLess(order.index("check-verification"), order.index("check-bridge-asan"))
        self.assertLess(order.index("check-bridge-asan"), order.index("check-product-contracts"))
        self.assertLess(order.index("check-tsan-runtime"), order.index("check-bridge-tsan"))
        checks = {c["id"]: c for c in policy["checks"]}
        self.assertIn("tsan-runtime", checks["check-bridge-tsan"]["preflight"])
        self.assertNotIn("tsan-runtime", checks["check-bridge-asan"]["preflight"])


class BridgeReadinessTests(unittest.TestCase):
    def test_pinned_source_and_miri_are_queried_without_installing_or_building(self):
        from unittest.mock import patch
        ready = support.module("check_bridge_ready", "check_bridge_ready.py")
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "scripts").mkdir()
            (root / "scripts/bridge-toolchain.env").write_text("BRIDGE_NIGHTLY=nightly-fixture\n")
            result = subprocess.CompletedProcess([], 0, stdout=str(root), stderr="")
            with patch.object(ready.subprocess, "run", return_value=result) as query:
                with self.assertRaisesRegex(ValueError, "rust-src"):
                    ready.check(root)
                source = root / "lib/rustlib/src/rust/library/Cargo.toml"
                source.parent.mkdir(parents=True)
                source.write_text("fixture")
                ready.check(root, miri=True)
                self.assertEqual(query.call_args_list[-1].args[0], ["cargo", "+nightly-fixture", "miri", "--version"])
                self.assertTrue(all(call.kwargs["env"]["RUSTUP_AUTO_INSTALL"] == "0"
                                    for call in query.call_args_list))
                query.side_effect = subprocess.CalledProcessError(1, "missing compiler")
                with self.assertRaises(subprocess.CalledProcessError):
                    ready.check(root)


class CheckedCommandTests(unittest.TestCase):
    def invoke(self, folder, *commands, extra=()):
        argv = [sys.executable, str(ROOT / "scripts/checked_commands.py"), "--log-dir", str(folder), *extra]
        for command in commands:
            argv += ["--command", command]
        return subprocess.run(argv, capture_output=True, text=True)

    def test_all_commands_run_but_any_failure_is_returned_and_logs_remain_complete(self):
        import shlex
        with tempfile.TemporaryDirectory() as temporary:
            folder = Path(temporary)
            fail = shlex.join([sys.executable, "-c", "print('failed assertion'); raise SystemExit(101)"])
            passed = shlex.join([sys.executable, "-c", "print('passed')"])
            result = self.invoke(folder, fail, passed)
            self.assertNotEqual(result.returncode, 0)
            data = json.loads((folder / "commands.json").read_text())
            self.assertEqual([r["exit_code"] for r in data], [101, 0])
            self.assertIn("failed assertion", Path(data[0]["log"]).read_text())
            self.assertEqual(self.invoke(folder, passed).returncode, 0)

    def test_expected_red_requires_both_exit_code_and_the_specific_assertion(self):
        import shlex
        with tempfile.TemporaryDirectory() as temporary:
            for code, text, expected in ((101, "assertion left != right", 0), (101, "compile error", 1),
                                         (0, "assertion left != right", 1), (7, "assertion left != right", 1)):
                command = shlex.join([sys.executable, "-c", f"print({text!r}); raise SystemExit({code})"])
                result = self.invoke(Path(temporary), command,
                                     extra=["--expect-exit", "101", "--expect-failure", "assertion left != right"])
                self.assertEqual(result.returncode, expected, result.stderr)

    def test_expected_failure_rejects_multiple_commands_and_timeouts(self):
        import shlex
        with tempfile.TemporaryDirectory() as temporary:
            command = shlex.join([sys.executable, "-c", "print('assertion', flush=True); import time; time.sleep(60)"])
            result = self.invoke(Path(temporary), command,
                                 extra=["--timeout", "0.1", "--expect-exit", "124", "--expect-failure", "assertion"])
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(json.loads((Path(temporary) / "commands.json").read_text())[0]["error"], "timeout")
            self.assertNotEqual(self.invoke(Path(temporary), command, command,
                                           extra=["--expect-exit", "101", "--expect-failure", "assertion"]).returncode, 0)


if __name__ == "__main__":
    unittest.main()
