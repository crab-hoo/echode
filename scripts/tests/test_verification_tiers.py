"""A task runs the checks that can catch its own change; GitHub's full workflow runs the rest (ADR 0212, 0327)."""

import io
import json
import subprocess
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

import test_verification as support

verify = support.verify
contracts = support.load_contracts()
ROOT = support.ROOT
FULL_TIER = {
    "check-bridge-asan",
    "check-bridge-tsan",
    "check-tsan-runtime",
    "check-bridge-miri",
    "check-contract-controls",
    "check-fuzz",
    "rtsan",
}


def git(root, *arguments):
    subprocess.run(
        ["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", *arguments],
        cwd=root,
        check=True,
        capture_output=True,
    )


def repository(root):
    git(root, "init", "-q")
    (root / ".gitignore").write_text("/target\n")


def commit(root):
    git(root, "add", ".")
    git(root, "commit", "-q", "-m", "base")


class RealPolicyTests(unittest.TestCase):
    def setUp(self):
        self.policy = verify.load_policy(ROOT)
        self.checks = {check["id"]: check for check in self.policy["checks"]}

    def test_the_full_tier_is_exactly_the_slow_evidence(self):
        self.assertEqual({name for name, check in self.checks.items() if check.get("tier") == "full"}, FULL_TIER)
        # Everything that catches an ordinary regression stays in every task.
        for name in (
            "check-format",
            "check-verification",
            "check",
            "check-swift",
            "check-product-contracts",
            "check-real-hal",
        ):
            self.assertTrue(self.checks[name].get("always") and self.checks[name].get("tier", "task") == "task", name)

    def test_ordinary_edits_leave_the_full_tier_out_of_a_task(self):
        for path in (
            "swift/EchodeApp/Sources/EchodeAppCore/PianoRollView.swift",
            "crates/echode-model/src/clip_move.rs",
            "crates/echode-ffi/src/lib.rs",
            "swift/EchodeBridge/Sources/EchodeBridge/Document.swift",
            "Cargo.lock",
            "scripts/verify.py",
            "scripts/verification.json",
            "Makefile",
            "docs/status.md",
        ):
            with self.subTest(path=path):
                self.assertFalse(FULL_TIER & set(verify.select_checks(self.policy, [path], redefined=None)))

    def test_no_change_selects_the_full_tier_in_a_task(self):
        # ADR 0327: instrumented and mutated builds run on GitHub, even for an unsafe edit or their own scripts.
        for path in (
            "scripts/check-bridge-address.sh",
            "scripts/check-bridge-miri.sh",
            "scripts/tsan_runtime.py",
            "scripts/check_product_contracts.py",
            "scripts/swift_contract_runner.py",
            "scripts/fuzz.sh",
            "crates/echode-fuzz/fuzz_targets/control.rs",
            "crates/echode-ffi/src/abi.rs",
            "crates/echode-engine/src/meter.rs",
            "spikes/bridge/sanitizers/Errno.c",
        ):
            with self.subTest(path=path):
                self.assertFalse(FULL_TIER & set(verify.select_checks(self.policy, [path], redefined=None)))
        self.assertFalse(FULL_TIER & set(verify.select_checks(self.policy, [], all_checks=False)))

    def test_the_full_tier_covers_every_automatic_check(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repository(root)
            (root / "note.txt").write_text("x\n")
            commit(root)
            policy = {
                **self.policy,
                "checks": [{k: v for k, v in check.items() if k != "packages"} for check in self.policy["checks"]],
            }
            selected = verify.plan_checks(root, policy, [], full=True)
        self.assertEqual({name for name, check in self.checks.items() if check["mode"] == "automatic"}, set(selected))

    def test_the_full_workflow_is_the_routine_run_of_the_full_tier(self):
        workflow = (ROOT / ".github/workflows/full.yml").read_text()
        self.assertIn("run: make rtsan", workflow)
        # make rtsan builds the bundled plugin offline first.
        self.assertLess(
            workflow.index("cargo fetch --locked", workflow.index("  rtsan:")), workflow.index("make rtsan")
        )

    def test_a_contract_edit_is_handled_inside_the_task_tier_gate_not_by_rerunning_every_control(self):
        self.assertIn(
            "check-product-contracts",
            verify.select_checks(
                self.policy,
                [verify.POLICY.as_posix()],
                redefined={"check-product-contracts", "check-contract-controls"},
            ),
        )
        self.assertNotIn(
            "check-contract-controls",
            verify.select_checks(
                self.policy,
                [verify.POLICY.as_posix()],
                redefined={"check-product-contracts", "check-contract-controls"},
            ),
        )

    def test_recipes_separate_the_task_tests_from_all_controls(self):
        makefile = (ROOT / "Makefile").read_text()
        task = makefile.split("\ncheck-product-contracts:\n", 1)[1].split("\n\n", 1)[0]
        full = makefile.split("\ncheck-contract-controls:\n", 1)[1].split("\n\n", 1)[0]
        self.assertIn("--tests-only", task)
        self.assertNotIn("--tests-only", full)
        self.assertIn("scripts/verify.py --full", makefile.split("\nverify-full:\n", 1)[1].split("\n\n", 1)[0])
        # A Swift-only change must not depend on what the Rust check built.
        swift = makefile.split("\ncheck-swift:\n", 1)[1].split("\n\n", 1)[0]
        self.assertIn("bundled-plugins", swift)
        self.assertIn("-p echode-ffi -p echode-plugin-host -p echode-test-plugin -p echode-cli", swift)

    def test_swift_edits_keep_the_rust_evidence_and_rust_edits_expire_both(self):
        stamps = verify.check_fingerprints(ROOT, self.policy, {}, {})
        digest = verify.input_digest

        def after(path):
            with patch.object(
                verify,
                "input_digest",
                side_effect=lambda root, relative: digest(root, relative) + (b"edited" if relative == path else b""),
            ):
                current = verify.check_fingerprints(ROOT, self.policy, {}, {})
            return {name for name in stamps if stamps[name] != current[name]}

        swift = after("swift/EchodeApp/Sources/EchodeAppCore/PianoRollView.swift")
        self.assertIn("check-swift", swift)
        self.assertNotIn("check", swift)
        rust = after("crates/echode-model/src/clip_move.rs")
        self.assertTrue({"check", "check-swift", "check-product-contracts"} <= rust)
        # The generated Swift actions are compared by the Rust check.
        self.assertIn("check", after("swift/EchodeBridge/Sources/EchodeBridge/EditAction.swift"))

    def test_the_c_abi_argument_rules_run_under_miri(self):
        # ADR 0321: the shared pointer checks and their tests also run under Miri, now in the full tier.
        self.assertIn("echode-ffi", self.checks["check-bridge-miri"]["packages"])
        self.assertIn("abi_tests", (ROOT / "scripts/check-bridge-miri.sh").read_text())

    def test_tiers_are_validated(self):
        def rejected(change):
            policy = json.loads(json.dumps(self.policy))
            change(next(check for check in policy["checks"] if check["id"] == "check-bridge-asan"))
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "scripts").mkdir()
                (root / verify.POLICY).write_text(json.dumps(policy))
                with self.assertRaisesRegex(ValueError, "tier|task_"):
                    verify.load_policy(root)

        rejected(lambda check: check.update(tier="sometimes"))
        rejected(lambda check: check.update(mode="manual"))
        # ADR 0327 removed the task triggers; a full-tier check that names one is refused.
        rejected(lambda check: check.update(task_requires=["check-tsan-runtime"]))
        rejected(lambda check: check.update(task_unsafe=True))
        rejected(lambda check: check.update(task_inputs=["scripts/check-bridge-address.sh"]))


class EvidenceTests(unittest.TestCase):
    setUp = support.EvidenceTests.setUp
    git = support.EvidenceTests.git
    report = support.EvidenceTests.report
    run_verify = support.EvidenceTests.run_verify

    def policy(self):
        policy = json.loads((self.root / verify.POLICY).read_text())
        policy["checks"] = [
            {
                "id": "check",
                "mode": "automatic",
                "always": True,
                "areas": [],
                "inputs": ["src/**"],
                "timeout": 10,
                "purpose": "fixture",
            },
            {
                "id": "slow",
                "mode": "automatic",
                "tier": "full",
                "areas": [],
                "inputs": ["src/**", "scripts/slow.sh"],
                "timeout": 10,
                "purpose": "fixture",
            },
        ]
        policy["preflights"] = {}
        (self.root / verify.POLICY).write_text(json.dumps(policy))

    def test_a_task_passes_without_the_full_tier_and_never_records_it_as_passed(self):
        self.policy()
        output = io.StringIO()
        with (
            patch.object(verify, "tool_identity", return_value={}),
            patch.object(verify, "run_check", return_value={"status": "passed"}) as invocation,
            redirect_stdout(output),
        ):
            self.assertEqual(verify.verify(self.root, support.arguments()), 0)
        self.assertEqual(invocation.call_count, 1)
        report = self.report()
        self.assertEqual(report["profile"], "task")
        self.assertNotIn("slow", report["results"])
        self.assertEqual(report["full_tier_unverified"], ["slow"])
        self.assertIn("全体検査で未確認", output.getvalue())
        self.assertIn("全体検査で未確認", (self.root / "target/verification/report.md").read_text())
        self.assertIn("`slow`", (self.root / "target/verification/report.md").read_text())

    def test_the_full_tier_runs_once_on_demand_and_is_reused_until_its_inputs_change(self):
        self.policy()
        self.assertEqual(self.run_verify(support.arguments()), (0, 1))
        self.assertEqual(self.run_verify(support.arguments(full=True)), (0, 1), "the task check is reused")
        report = self.report()
        self.assertEqual(
            (report["profile"], report["results"]["slow"]["status"], report["full_tier_unverified"]),
            ("full", "passed", []),
        )
        self.assertTrue(report["results"]["slow"]["finished_at"])
        self.assertEqual(self.run_verify(support.arguments(full=True)), (0, 0))
        (self.root / "src/app.txt").write_text("edited\n")
        self.assertEqual(self.run_verify(support.arguments()), (0, 1))
        self.assertEqual(self.report()["full_tier_unverified"], ["slow"], "a task must not keep an outdated pass")

    def test_full_verification_of_a_clean_tree_still_runs(self):
        self.policy()
        self.git("add", ".")
        self.git(
            "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--quiet", "-m", "clean"
        )
        self.assertEqual(self.run_verify(support.arguments()), (0, 0), "no change selects nothing in a task")
        self.assertEqual(self.run_verify(support.arguments(full=True)), (0, 2))

    def test_an_edit_to_a_full_tier_input_leaves_it_to_the_full_tier(self):
        self.policy()
        (self.root / "scripts/slow.sh").write_text("true\n")
        seen = []

        def run(root, check, log, force=False, base=None):
            seen.append((check["id"], base))
            return {"status": "passed", "seconds": 0, "log": str(log)}

        self.assertEqual(self.run_verify(support.arguments(), run), (0, 1))
        self.assertEqual(seen, [("check", None)])
        report = self.report()
        self.assertNotIn("slow", report["selected"])
        self.assertEqual(report["full_tier_unverified"], ["slow"], "an edit expires the full tier's pass")
        seen.clear()
        self.assertEqual(self.run_verify(support.arguments(full=True), run), (0, 1))
        self.assertEqual(seen, [("slow", None)], "the task check is reused")
        self.assertEqual(self.report()["full_tier_unverified"], [])


class CounterexampleTests(unittest.TestCase):
    def fixture(self, root):
        repository(root)
        (root / "src").mkdir()
        (root / "tests").mkdir()
        padding = "".join(f"// line {index}\n" for index in range(60))
        (root / "src/lib.rs").write_text(padding + "let defect = false;\n" + padding)
        (root / "tests/case.rs").write_text(
            "#[test]\nfn first() {\n    assert!(true);\n}\n\n#[test]\nfn second() {\n    assert!(true);\n}\n"
        )
        self.cases = [
            {
                "id": name,
                "runner": "rust",
                "package": "p",
                "target": ["--test", "case"],
                "test": name,
                "timeout": 5,
                "purpose": "fixture",
                "negative_control": {"file": "src/lib.rs", "old": "let defect = false;", "new": "let defect = true;"},
            }
            for name in ("first", "second")
        ]
        self.policy = {"product_contracts": self.cases}
        (root / "scripts").mkdir()
        (root / verify.POLICY).write_text(json.dumps(self.policy))
        commit(root)
        return padding

    def test_a_counterexample_that_no_longer_applies_is_rejected_without_a_build(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            padding = self.fixture(root)
            contracts.reject_unmatched_controls(root, self.cases)
            (root / "src/lib.rs").write_text(padding + padding)
            with self.assertRaisesRegex(ValueError, "no longer matches: first, second"):
                contracts.reject_unmatched_controls(root, self.cases)

    def run_gate(self, root, tests_only=True, fresh=False):
        tested, controlled = [], []

        def run_tests(_root, pending, _invoke, stamps, _evidence, record, batched=False):
            for case in pending:
                tested.append(case["id"])
                isolation = {"isolation": "batch"} if batched else {}
                record({"id": case["id"], "status": "passed", "fingerprint": stamps[case["id"]], **isolation})

        def run_controls(_root, pending, stamps, _evidence, record):
            for case in pending:
                controlled.append(case["id"])
                record({"id": case["id"], "status": "passed", "fingerprint": stamps[case["id"]]})

        with (
            patch.object(contracts, "run_tests", side_effect=run_tests),
            patch.object(contracts, "negative_controls", side_effect=run_controls),
            patch.object(
                contracts,
                "contract_fingerprints",
                side_effect=lambda _root, _policy, cases, _tools, phase="complete": {
                    case["id"]: f"{self.generation}:{phase}" for case in cases
                },
            ),
            patch.object(contracts.verify, "tool_identity", return_value={}),
            redirect_stdout(io.StringIO()),
        ):
            code = contracts.run(root, self.cases, policy=self.policy, tests_only=tests_only, fresh=fresh)
        return code, tested, controlled

    def report(self, root):
        return json.loads((root / "target/verification/product-contracts.json").read_text())

    def test_a_task_runs_every_test_and_leaves_every_counterexample_to_the_full_tier(self):
        # ADR 0327: even an edit beside the mutated text runs no counterexample in a task.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            padding = self.fixture(root)
            self.generation = 1
            self.assertEqual(self.run_gate(root, tests_only=False), (0, ["first", "second"], ["first", "second"]))
            self.generation = 2
            (root / "src/lib.rs").write_text(padding + "let beside = 1;\nlet defect = false;\n" + padding)
            self.assertEqual(self.run_gate(root), (0, ["first", "second"], []))
            report = self.report(root)
            self.assertEqual(
                (report["status"], report["execution"]["control_scope"], report["execution"]["negative_pending"]),
                ("passed", "none", 0),
            )
            self.assertEqual(report["negative_controls"], [], "an outdated counterexample is not kept as a pass")
            self.assertEqual(self.run_gate(root), (0, [], []), "the task's tests are reused")
            # The full tier proves each test alone and runs every counterexample on the current inputs.
            self.assertEqual(self.run_gate(root, tests_only=False), (0, ["first", "second"], ["first", "second"]))
            self.assertEqual(self.run_gate(root, tests_only=False), (0, [], []))
            self.assertEqual(self.run_gate(root), (0, [], []), "a task keeps the current counterexample passes")
            self.assertEqual(len(self.report(root)["negative_controls"]), 2)

    def test_every_real_counterexample_applies_and_its_test_exists(self):
        policy = verify.load_policy(ROOT)
        contracts.reject_unmatched_controls(ROOT, policy["product_contracts"])
        texts = {}
        for contract in policy["product_contracts"]:
            key = contract.get("package_path") or contract["package"]
            sources = texts.setdefault(key, [path.read_text(errors="replace") for path in self.sources_of(contract)])
            self.assertTrue(any(contracts.test_body(text, contract) for text in sources), contract["id"])

    def sources_of(self, contract):
        if contract["runner"] == "swift":
            return (ROOT / contract["package_path"]).glob("Tests/**/*.swift")
        folder = verify.workspace_members(ROOT)[contract["package"]]
        return [*folder.glob("src/**/*.rs"), *folder.glob("tests/**/*.rs")]

    def test_a_copy_that_does_not_build_stops_before_any_counterexample(self):
        case = {
            "id": "a",
            "runner": "swift",
            "package_path": "swift/App",
            "build_product": "Tests",
            "suite": "S",
            "test": "a",
            "timeout": 180,
            "purpose": "fixture",
            "negative_control": {"file": "a", "old": "b", "new": "c"},
        }
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            calls = []

            def built(args, **kwargs):
                calls.append(kwargs["timeout"])
                return subprocess.CompletedProcess(args, 0, "", "")

            with patch.object(contracts, "run_process", side_effect=built):
                contracts.build_swift_copies([root, root], [case])
            self.assertEqual(
                calls,
                [contracts.SWIFT_COPY_BUILD_TIMEOUT] * 2,
                "bringing a copy up to date is not charged to a contract's deadline",
            )

            def broken(args, **kwargs):
                return subprocess.CompletedProcess(args, 1, "", "error: no such module")

            with (
                patch.object(contracts, "run_process", side_effect=broken),
                self.assertRaisesRegex(ValueError, "does not build:\nerror: no such module"),
            ):
                contracts.build_swift_copies([root], [case])


if __name__ == "__main__":
    unittest.main()
