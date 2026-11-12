"""Execution safety, selected-only prerequisites, and failure detection order."""

import io
import json
import shlex
import subprocess
import sys
import tempfile
import unittest
import unittest.mock
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

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
        return {
            "id": name,
            "mode": "automatic",
            "always": True,
            "areas": [],
            "inputs": ["src/**"],
            "timeout": 10,
            "purpose": "fixture",
            **extra,
        }

    def test_make_recipes_cannot_hide_pipeline_or_intermediate_failures(self):
        for recipe in ("false | cat", "false; echo status=$?", "false; true"):
            with self.subTest(recipe=recipe):
                (self.root / "Makefile").write_text("check:\n\t@" + recipe + "\n")
                result = verify.run_check(self.root, self.check("check"), self.root / "run.log")
                self.assertEqual(result["status"], "failed", "an earlier failure was hidden")

    def test_order_is_stable_topological_and_preserves_the_requested_set(self):
        checks = [
            self.check("slow"),
            self.check("asan", priority=30),
            self.check("tsan", priority=0, after=["runtime"]),
            self.check("runtime", priority=20),
            self.check("infra", priority=10),
        ]
        policy = self.policy(checks)
        self.assertEqual(
            [c["id"] for c in verify.execution_order(policy, {c["id"] for c in checks})],
            ["infra", "runtime", "tsan", "asan", "slow"],
        )
        self.assertEqual([c["id"] for c in verify.execution_order(policy, {"tsan", "slow"})], ["tsan", "slow"])
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
        self.policy(
            [self.check("check", preflight=["runtime"])],
            {
                "runtime": {
                    "command": [sys.executable, "-c", "raise SystemExit(7)"],
                    "timeout": 5,
                    "remedy": "make setup-bridge",
                }
            },
        )
        self.assertEqual(self.run_verify(support.arguments()), (1, 0))
        report = self.report()
        self.assertEqual(report["results"]["check"]["status"], "pending")
        self.assertEqual(report["preflight"]["runtime"]["exit_code"], 7)
        self.assertIn("make setup-bridge", (self.root / "target/verification/report.md").read_text())
        self.assertEqual(report["execution"]["first_failure"]["kind"], "preflight")
        self.assertGreaterEqual(report["execution"]["first_failure"]["seconds"], 0)

    def test_prerequisites_only_cover_uncached_requested_executions_and_are_deduplicated(self):
        counter = self.root / "target/preflight-count"
        command = [
            sys.executable,
            "-c",
            (
                f"from pathlib import Path; p=Path({str(counter)!r}); "
                "p.write_text(p.read_text()+'x' if p.exists() else 'x')"
            ),
        ]
        checks = [
            self.check("check", preflight=["ready"]),
            self.check("other", preflight=["ready"]),
            self.check("measure", mode="manual", preflight=["broken"]),
        ]
        self.policy(
            checks,
            {
                "ready": {"command": command, "timeout": 5, "remedy": "prepare fixture"},
                "broken": {"command": ["missing-fixture-tool"], "timeout": 5, "remedy": "prepare fixture"},
            },
        )
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
        self.policy(
            [self.check("check", preflight=["ready"])],
            {"ready": {"command": [sys.executable, "scripts/ready.py"], "timeout": 5, "remedy": "prepare fixture"}},
        )
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

    def test_each_deferral_keeps_its_own_reason_and_a_shared_reason_is_refused(self):
        self.policy([self.check("check"), self.check("measure", mode="manual"), self.check("listen", mode="manual")])
        output = io.StringIO()
        with redirect_stdout(output):
            code, count = self.run_verify(
                support.arguments(defer=["measure=no device", "listen=the user listens = later"])
            )
        self.assertEqual((code, count), (0, 1))
        results = self.report()["results"]
        self.assertEqual(
            (results["measure"]["reason"], results["listen"]["reason"]), ("no device", "the user listens = later")
        )
        self.assertRegex(results["measure"]["deferred_at"], r"^\d{4}-\d\d-\d\dT")
        self.assertIn("未実行の理由: no device（記録 ", (self.root / "target/verification/report.md").read_text())
        for refused in (
            {"defer": ["measure", "listen"], "reason": "shared"},
            {"defer": ["measure=a", "measure=b"]},
            {"defer": ["measure= "]},
            {"defer": ["measure=a"], "reason": "stray"},
            {"defer": ["measure"]},
        ):
            with self.subTest(refused=refused), self.assertRaises(ValueError):
                self.run_verify(support.arguments(**refused))
        # One plain ID with --reason keeps working for existing callers.
        self.assertEqual(self.run_verify(support.arguments(defer=["measure"], reason="single")), (0, 0))
        self.assertEqual(self.report()["results"]["listen"]["reason"], "the user listens = later")

    def test_every_selected_deferral_and_its_reason_reach_the_final_output(self):
        self.policy([self.check("check"), self.check("measure", mode="manual")])
        self.assertEqual(self.run_verify(support.arguments(defer=["measure=no device"])), (0, 1))
        output = io.StringIO()
        with patch.object(verify, "tool_identity", return_value={"rust": "fixture-v1"}), redirect_stdout(output):
            self.assertEqual(verify.verify(self.root, support.arguments()), 0)
        self.assertRegex(output.getvalue(), r"見送り measure: no device（記録 \d{4}-")

    def test_record_only_writes_deferrals_without_preflights_checks_or_timing(self):
        self.policy(
            [self.check("check", preflight=["broken"]), self.check("measure", mode="manual")],
            {"broken": {"command": ["missing-fixture-tool"], "timeout": 5, "remedy": "prepare fixture"}},
        )
        self.assertEqual(self.run_verify(support.arguments(defer=["measure=no device"], record_only=True)), (0, 0))
        report = self.report()
        self.assertEqual(
            (report["results"]["check"]["status"], report["results"]["measure"]["status"]), ("pending", "deferred")
        )
        self.assertEqual((report["execution"]["status"], report["preflight"]), ("recorded", {}))
        self.assertFalse((self.root / "target/verification/timing-history.jsonl").exists())
        self.assertIn("--record-only", (self.root / "target/verification/report.md").read_text())
        for refused in (
            {"record_only": True},
            {"record_only": True, "defer": ["measure=x"], "run": ["measure"]},
            {"record_only": True, "defer": ["measure=x"], "only": ["measure"]},
            {"record_only": True, "defer": ["measure=x"], "force": True},
        ):
            with self.subTest(refused=refused), self.assertRaises(ValueError):
                self.run_verify(support.arguments(**refused))
        self.policy([self.check("check"), self.check("measure", mode="manual")])
        self.assertEqual(self.run_verify(support.arguments()), (0, 1), "the recorded deferral completes a later verify")

    def test_display_only_make_flags_keep_the_identity_but_outcome_flags_change_it(self):
        self.assertEqual(verify.make_flags_identity("s"), [])
        self.assertEqual(verify.make_flags_identity("ws -- VERIFY_ARGS=--plan"), [])
        self.assertEqual(verify.make_flags_identity(" --no-print-directory"), [])
        self.assertEqual(verify.make_flags_identity("s --jobserver-fds=3,4 -j"), ["-j"])
        self.assertEqual(verify.make_flags_identity("ski"), ["ki"])
        self.assertEqual(verify.make_flags_identity("VERIFY_ARGS=x"), [])
        # The host's make decides how flags are spelled; compare real invocations.
        (self.root / "Makefile").write_text(
            "identity:\n\t@"
            + shlex.quote(sys.executable)
            + " -B -c "
            + shlex.quote(
                "import json, os, sys; sys.path.insert(0, "
                + repr(str(ROOT / "scripts"))
                + "); import verify; print(json.dumps(verify.make_flags_identity("
                "os.environ.get('MAKEFLAGS', ''))))"
            )
            + "\n"
        )

        def identity(*flags):
            result = subprocess.run(
                ["make", *flags, "identity"], cwd=self.root, capture_output=True, text=True, check=True
            )
            return [line for line in result.stdout.splitlines() if line.startswith("[")][-1]

        plain = identity()
        for flags in (["-s"], ["-w"], ["--no-print-directory"], ["-C", str(self.root)], ["-s", "VERIFY_ARGS=--plan"]):
            with self.subTest(flags=flags):
                self.assertEqual(identity(*flags), plain)
        for flags in (["-i"], ["-k"], ["-s", "-i"]):
            with self.subTest(flags=flags):
                self.assertNotEqual(identity(*flags), plain)

    def test_build_checks_require_free_space_sized_for_their_cold_caches(self):
        disk = support.module("check_disk_space", "check_disk_space.py")
        (self.root / "target/debug").mkdir(parents=True)
        cold = [("target/debug", 6.0), ("target/mutations", 4.0)]
        self.assertEqual(disk.required_gib(self.root, 3.0, cold), (7.0, ["target/mutations"]))
        usage = lambda free: unittest.mock.patch.object(
            disk.shutil, "disk_usage", return_value=SimpleNamespace(free=free * disk.GIB)
        )
        with usage(6.9), redirect_stderr(io.StringIO()) as error:
            self.assertEqual(
                disk.main(["--min-gib", "3", "--cold", "target/debug=6", "--cold", "target/mutations=4"], self.root), 1
            )
        self.assertIn("target/mutations", error.getvalue())
        with usage(7.1), redirect_stdout(io.StringIO()):
            self.assertEqual(disk.main(["--min-gib", "3", "--cold", "target/mutations=4"], self.root), 0)
        for bad in ("/tmp/x=1", "../x=1", "target=0", "target"):
            with self.subTest(bad=bad), redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                disk.main(["--min-gib", "3", "--cold", bad], self.root)
        policy = verify.load_policy(ROOT)
        checks = {c["id"]: c for c in policy["checks"]}
        mutations = policy["preflights"]["disk-space-mutations"]["command"]
        self.assertIn(f"{support.load_contracts().MUTATIONS.as_posix()}/cargo=4", mutations)
        for name, check in checks.items():
            if check["mode"] == "automatic" and name not in (
                "check-format",
                "check-lint",
                "check-verification",
                "check-tsan-runtime",
            ):
                with self.subTest(check=name):
                    expected = {
                        "check-product-contracts": "disk-space-mutations",
                        "check-contract-controls": "disk-space-mutations",
                        "check-fuzz": "disk-space-fuzz",
                    }.get(name, "disk-space")
                    self.assertEqual(check["preflight"][0], expected)
        # The fuzz build and the instrumented coverage build have their own cold sizes (ADR 0290).
        self.assertIn("target/aarch64-apple-darwin=2", policy["preflights"]["disk-space-fuzz"]["command"])
        self.assertEqual(checks["coverage"]["preflight"][0], "disk-space-coverage")
        self.assertIn("target/llvm-cov-target=7", policy["preflights"]["disk-space-coverage"]["command"])


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
                self.assertTrue(all(call.kwargs["env"]["RUSTUP_AUTO_INSTALL"] == "0" for call in query.call_args_list))
                query.side_effect = subprocess.CalledProcessError(1, "missing compiler")
                with self.assertRaises(subprocess.CalledProcessError):
                    ready.check(root)


class CheckedCommandTests(unittest.TestCase):
    def invoke(self, folder, *commands, extra=()):
        argv = [sys.executable, str(ROOT / "scripts/checked_commands.py"), "--log-dir", str(folder), *extra]
        for command in commands:
            argv += ["--command", command]
        return subprocess.run(argv, capture_output=True, text=True, check=False)

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
            for code, text, expected in (
                (101, "assertion left != right", 0),
                (101, "compile error", 1),
                (0, "assertion left != right", 1),
                (7, "assertion left != right", 1),
            ):
                command = shlex.join([sys.executable, "-c", f"print({text!r}); raise SystemExit({code})"])
                result = self.invoke(
                    Path(temporary),
                    command,
                    extra=["--expect-exit", "101", "--expect-failure", "assertion left != right"],
                )
                self.assertEqual(result.returncode, expected, result.stderr)

    def test_expected_failure_rejects_multiple_commands_and_timeouts(self):
        import shlex

        with tempfile.TemporaryDirectory() as temporary:
            command = shlex.join([sys.executable, "-c", "print('assertion', flush=True); import time; time.sleep(60)"])
            result = self.invoke(
                Path(temporary),
                command,
                extra=["--timeout", "0.1", "--expect-exit", "124", "--expect-failure", "assertion"],
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(json.loads((Path(temporary) / "commands.json").read_text())[0]["error"], "timeout")
            self.assertNotEqual(
                self.invoke(
                    Path(temporary), command, command, extra=["--expect-exit", "101", "--expect-failure", "assertion"]
                ).returncode,
                0,
            )


if __name__ == "__main__":
    unittest.main()
