"""A task runs the checks that can catch its own change; the full tier runs the rest (ADR 0212)."""

from contextlib import redirect_stderr, redirect_stdout
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import test_verification as support

verify = support.verify
contracts = support.load_contracts()
ROOT = support.ROOT
FULL_TIER = {"check-bridge-asan", "check-bridge-tsan", "check-tsan-runtime", "check-bridge-miri", "check-contract-controls", "check-fuzz"}


def git(root, *arguments):
    subprocess.run(["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", *arguments],
                   cwd=root, check=True, capture_output=True)


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
        for name in ("check-format", "check-verification", "check", "check-swift", "check-product-contracts", "check-real-hal"):
            self.assertTrue(self.checks[name].get("always") and self.checks[name].get("tier", "task") == "task", name)

    def test_ordinary_edits_leave_the_full_tier_out_of_a_task(self):
        for path in ("swift/EchodeApp/Sources/EchodeAppCore/PianoRollView.swift", "crates/echode-model/src/clip_move.rs",
                     "crates/echode-ffi/src/lib.rs", "swift/EchodeBridge/Sources/EchodeBridge/Document.swift",
                     "Cargo.lock", "scripts/verify.py", "scripts/verification.json", "Makefile", "docs/status.md"):
            with self.subTest(path=path):
                self.assertFalse(FULL_TIER & set(verify.select_checks(self.policy, [path], redefined=None)))

    def test_changing_a_full_tier_check_itself_selects_it_in_a_task(self):
        for path, expected in (("scripts/check-bridge-address.sh", {"check-bridge-asan", "check-bridge-tsan", "check-tsan-runtime"}),
                               ("scripts/check-bridge-miri.sh", {"check-bridge-miri"}),
                               ("scripts/tsan_runtime.py", {"check-bridge-tsan", "check-tsan-runtime"}),
                               ("scripts/check_product_contracts.py", {"check-contract-controls"}),
                               ("scripts/swift_contract_runner.py", {"check-contract-controls"}),
                               ("scripts/fuzz.sh", {"check-fuzz"}),
                               ("crates/echode-fuzz/fuzz_targets/control.rs", {"check-fuzz"})):
            with self.subTest(path=path):
                self.assertEqual(FULL_TIER & set(verify.select_checks(self.policy, [path])), expected)

    def test_tsan_in_a_task_brings_its_runtime_control_and_the_full_tier_covers_every_automatic_check(self):
        selected = verify.select_checks(self.policy, [], unsafe={"check-bridge-tsan": ["crates/echode-ffi/src/lib.rs"]})
        self.assertEqual(set(selected), {"check-bridge-tsan", "check-tsan-runtime"})
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            repository(root)
            (root / "note.txt").write_text("x\n")
            commit(root)
            policy = {**self.policy, "checks": [{k: v for k, v in check.items() if k != "packages"}
                                                for check in self.policy["checks"]]}
            selected = verify.plan_checks(root, policy, [], full=True)
        self.assertEqual({name for name, check in self.checks.items() if check["mode"] == "automatic"}, set(selected))

    def test_a_contract_edit_is_handled_inside_the_task_tier_gate_not_by_rerunning_every_control(self):
        self.assertIn("check-product-contracts", verify.select_checks(self.policy, [verify.POLICY.as_posix()],
                                                                      redefined={"check-product-contracts", "check-contract-controls"}))
        self.assertNotIn("check-contract-controls", verify.select_checks(self.policy, [verify.POLICY.as_posix()],
                                                                         redefined={"check-product-contracts", "check-contract-controls"}))

    def test_recipes_separate_changed_controls_from_all_controls(self):
        makefile = (ROOT / "Makefile").read_text()
        task = makefile.split("\ncheck-product-contracts:\n", 1)[1].split("\n\n", 1)[0]
        full = makefile.split("\ncheck-contract-controls:\n", 1)[1].split("\n\n", 1)[0]
        self.assertIn("--changed-controls", task)
        self.assertNotIn("--changed-controls", full)
        self.assertIn("scripts/verify.py --full", makefile.split("\nverify-full:\n", 1)[1].split("\n\n", 1)[0])
        # A Swift-only change must not depend on what the Rust check built.
        swift = makefile.split("\ncheck-swift:\n", 1)[1].split("\n\n", 1)[0]
        self.assertIn("bundled-plugins", swift)
        self.assertIn("-p echode-ffi -p echode-plugin-host -p echode-test-plugin -p echode-cli", swift)

    def test_swift_edits_keep_the_rust_evidence_and_rust_edits_expire_both(self):
        stamps = verify.check_fingerprints(ROOT, self.policy, {}, {})
        digest = verify.input_digest
        def after(path):
            with patch.object(verify, "input_digest", side_effect=lambda root, relative:
                              digest(root, relative) + (b"edited" if relative == path else b"")):
                current = verify.check_fingerprints(ROOT, self.policy, {}, {})
            return {name for name in stamps if stamps[name] != current[name]}
        swift = after("swift/EchodeApp/Sources/EchodeAppCore/PianoRollView.swift")
        self.assertIn("check-swift", swift)
        self.assertNotIn("check", swift)
        rust = after("crates/echode-model/src/clip_move.rs")
        self.assertTrue({"check", "check-swift", "check-product-contracts"} <= rust)
        # The generated Swift actions are compared by the Rust check.
        self.assertIn("check", after("swift/EchodeBridge/Sources/EchodeBridge/EditAction.swift"))

    def test_every_task_trigger_names_a_file_that_exists(self):
        files = verify.source_files(ROOT)
        for check in self.policy["checks"]:
            for pattern in check.get("task_inputs", []):
                with self.subTest(check=check["id"], pattern=pattern):
                    self.assertTrue(verify.matches(files, [pattern]), "a task trigger that matches nothing never fires")

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
        rejected(lambda check: check.update(tier="task"))
        rejected(lambda check: check.update(task_requires=["check"]))


class UnsafeChangeTests(unittest.TestCase):
    def fixture(self, root):
        repository(root)
        (root / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/*"]\n')
        for name in ("linked", "other"):
            (root / f"crates/{name}/src").mkdir(parents=True)
            (root / f"crates/{name}/tests").mkdir()
            (root / f"crates/{name}/Cargo.toml").write_text(f'[package]\nname = "{name}"\nversion = "0.1.0"\n')
            (root / f"crates/{name}/src/lib.rs").write_text("pub fn safe() {}\nunsafe fn raw() {}\n")
            (root / f"crates/{name}/tests/case.rs").write_text("fn case() {}\n")
        (root / "swift/Sources").mkdir(parents=True)
        (root / "swift/Tests").mkdir()
        (root / "swift/Sources/View.swift").write_text("struct View {}\nfinal class Box: @unchecked Sendable {}\n")
        (root / "swift/Tests/Case.swift").write_text("struct Case {}\n")
        commit(root)
        return {"checks": [{"id": "sanitizer", "mode": "automatic", "tier": "full", "task_unsafe": True, "areas": [],
                            "timeout": 5, "packages": ["linked"], "inputs": ["swift/Sources/**", "swift/Tests/**"]}]}

    def found(self, root, policy, base=None):
        return verify.unsafe_changes(root, policy, verify.changed_paths(root, base), base).get("sanitizer", [])

    def test_an_edit_beside_existing_unsafe_code_does_not_count(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            policy = self.fixture(root)
            (root / "crates/linked/src/lib.rs").write_text("pub fn safe() { }\nunsafe fn raw() {}\n")
            (root / "swift/Sources/View.swift").write_text("struct View { }\nfinal class Box: @unchecked Sendable {}\n")
            self.assertEqual(self.found(root, policy), [])

    def test_added_and_removed_escapes_count_within_the_check_scope(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            policy = self.fixture(root)
            (root / "crates/linked/src/lib.rs").write_text("pub fn safe() {}\n")
            (root / "swift/Sources/View.swift").write_text(
                "struct View {}\nfinal class Box: @unchecked Sendable {}\nlet p = withUnsafeBytes(of: 1) { _ in }\n")
            self.assertEqual(self.found(root, policy), ["crates/linked/src/lib.rs", "swift/Sources/View.swift"])

    def test_a_new_file_counts_but_other_crates_and_tests_do_not(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            policy = self.fixture(root)
            (root / "crates/linked/src/new.rs").write_text("unsafe fn added() {}\n")
            (root / "crates/other/src/lib.rs").write_text("unsafe fn elsewhere() {}\n")
            (root / "crates/linked/tests/case.rs").write_text("unsafe fn fixture() {}\n")
            (root / "swift/Tests/Case.swift").write_text("final class Case: @unchecked Sendable {}\n")
            self.assertEqual(self.found(root, policy), ["crates/linked/src/new.rs"])

    def test_the_diff_base_decides_and_an_unreadable_base_falls_back_to_the_whole_file(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            policy = self.fixture(root)
            subprocess.run(["git", "tag", "start"], cwd=root, check=True)
            (root / "crates/linked/src/lib.rs").write_text("pub fn safe() {}\nunsafe fn raw() {}\nunsafe fn more() {}\n")
            commit(root)
            self.assertEqual(self.found(root, policy), [])
            self.assertEqual(self.found(root, policy, "start"), ["crates/linked/src/lib.rs"])
            (root / "crates/linked/src/lib.rs").write_text("pub fn other() {}\nunsafe fn raw() {}\nunsafe fn more() {}\n")
            with patch.object(verify, "changed_lines", side_effect=subprocess.CalledProcessError(128, "git")):
                self.assertEqual(self.found(root, policy), ["crates/linked/src/lib.rs"])


class EvidenceTests(unittest.TestCase):
    setUp = support.EvidenceTests.setUp
    git = support.EvidenceTests.git
    report = support.EvidenceTests.report
    run_verify = support.EvidenceTests.run_verify

    def policy(self):
        policy = json.loads((self.root / verify.POLICY).read_text())
        policy["checks"] = [{"id": "check", "mode": "automatic", "always": True, "areas": [], "inputs": ["src/**"],
                             "timeout": 10, "purpose": "fixture"},
                            {"id": "slow", "mode": "automatic", "tier": "full", "task_inputs": ["scripts/slow.sh"],
                             "areas": [], "inputs": ["src/**"], "timeout": 10, "purpose": "fixture"}]
        policy["preflights"] = {}
        policy.pop("hook_timeout", None)
        (self.root / verify.POLICY).write_text(json.dumps(policy))

    def test_a_task_passes_without_the_full_tier_and_never_records_it_as_passed(self):
        self.policy()
        output = io.StringIO()
        with patch.object(verify, "tool_identity", return_value={}), \
             patch.object(verify, "run_check", return_value={"status": "passed"}) as invocation, redirect_stdout(output):
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
        self.assertEqual((report["profile"], report["results"]["slow"]["status"], report["full_tier_unverified"]),
                         ("full", "passed", []))
        self.assertTrue(report["results"]["slow"]["finished_at"])
        self.assertEqual(self.run_verify(support.arguments(full=True)), (0, 0))
        (self.root / "src/app.txt").write_text("edited\n")
        self.assertEqual(self.run_verify(support.arguments()), (0, 1))
        self.assertEqual(self.report()["full_tier_unverified"], ["slow"], "a task must not keep an outdated pass")

    def test_full_verification_of_a_clean_tree_still_runs(self):
        self.policy()
        self.git("add", ".")
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--quiet", "-m", "clean")
        self.assertEqual(self.run_verify(support.arguments()), (0, 0), "no change selects nothing in a task")
        self.assertEqual(self.run_verify(support.arguments(full=True)), (0, 2))

    def test_a_passed_trigger_left_uncommitted_does_not_rerun_on_later_edits(self):
        self.policy()
        trigger = self.root / "scripts/slow.sh"
        trigger.write_text("true\n")
        self.git("add", "scripts/slow.sh")
        self.assertEqual(self.run_verify(support.arguments()), (0, 2))
        (self.root / "src/app.txt").write_text("another task\n")
        self.assertEqual(self.run_verify(support.arguments()), (0, 1), "only the task-tier check reruns")
        report = self.report()
        self.assertNotIn("slow", report["selected"])
        self.assertEqual(report["full_tier_unverified"], ["slow"], "an outdated pass is still not a current pass")
        trigger.write_text("false\n")
        self.assertEqual(self.run_verify(support.arguments()), (0, 1), "an edit to the trigger itself selects it again")
        self.assertEqual(self.report()["results"]["slow"]["status"], "passed")

    def test_a_failed_run_covers_nothing(self):
        self.policy()
        (self.root / "scripts/slow.sh").write_text("true\n")
        def run(root, check, log, force=False, base=None):
            return {"status": "failed" if check["id"] == "slow" else "passed", "seconds": 0, "log": str(log)}
        self.assertEqual(self.run_verify(support.arguments(), run), (1, 2))
        (self.root / "src/app.txt").write_text("another edit\n")
        self.assertEqual(self.run_verify(support.arguments()), (0, 2))

    def test_a_task_trigger_runs_the_check_and_a_base_reaches_the_child(self):
        self.policy()
        (self.root / "scripts/slow.sh").write_text("true\n")
        seen = []
        def run(root, check, log, force=False, base=None):
            seen.append((check["id"], base))
            return {"status": "passed", "seconds": 0, "log": str(log)}
        self.assertEqual(self.run_verify(support.arguments(), run), (0, 2))
        self.assertEqual(seen, [("check", None), ("slow", None)])
        self.assertEqual(self.report()["full_tier_unverified"], [])


class ControlSiteTests(unittest.TestCase):
    def fixture(self, root):
        repository(root)
        (root / "src").mkdir()
        (root / "tests").mkdir()
        padding = "".join(f"// line {index}\n" for index in range(60))
        (root / "src/lib.rs").write_text(padding + "let defect = false;\n" + padding)
        (root / "tests/case.rs").write_text("#[test]\nfn first() {\n    assert!(true);\n}\n\n#[test]\nfn second() {\n    assert!(true);\n}\n")
        self.cases = [{"id": name, "runner": "rust", "package": "p", "target": ["--test", "case"], "test": name, "timeout": 5,
                       "purpose": "fixture", "negative_control": {"file": "src/lib.rs", "old": "let defect = false;", "new": "let defect = true;"}}
                      for name in ("first", "second")]
        self.policy = {"product_contracts": self.cases}
        (root / "scripts").mkdir()
        (root / verify.POLICY).write_text(json.dumps(self.policy))
        commit(root)
        return padding

    def targets(self, root, base=None):
        return contracts.control_sites(root, self.policy, self.cases, base)[1]

    def test_only_a_changed_site_or_definition_selects_a_counterexample(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            padding = self.fixture(root)
            self.assertEqual(self.targets(root), set())
            source = root / "src/lib.rs"
            source.write_text("// a distant edit\n" + padding + "let defect = false;\n" + padding + "// and another\n")
            self.assertEqual(self.targets(root), set(), "an edit far from the mutation leaves its evidence meaningful")
            source.write_text(padding + "let beside = 1;\nlet defect = false;\n" + padding)
            self.assertEqual(self.targets(root), {"first", "second"})
            source.write_text(padding + "let defect = false;\n" + padding)
            tests = root / "tests/case.rs"
            tests.write_text(tests.read_text().replace("fn second() {\n    assert!(true);", "fn second() {\n    assert!(false);"))
            self.assertEqual(self.targets(root), {"second"}, "another test in the same file is unaffected")
            subprocess.run(["git", "checkout", "-q", "tests/case.rs"], cwd=root, check=True)
            self.cases[0]["purpose"] = "a better explanation"
            self.assertEqual(self.targets(root), set())
            self.cases[0]["negative_control"] = {**self.cases[0]["negative_control"], "new": "let defect = 1;"}
            self.assertEqual(self.targets(root), {"first"})
            self.cases.append({**self.cases[1], "id": "added", "test": "added"})
            self.assertEqual(self.targets(root), {"first", "added"}, "a new contract must prove its counterexample")

    def test_committed_edits_count_against_an_explicit_base(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            padding = self.fixture(root)
            subprocess.run(["git", "tag", "start"], cwd=root, check=True)
            (root / "src/lib.rs").write_text(padding + "let beside = 1;\nlet defect = false;\n" + padding)
            commit(root)
            self.assertEqual(self.targets(root), set())
            self.assertEqual(self.targets(root, "start"), {"first", "second"})

    def test_a_counterexample_that_no_longer_applies_is_rejected_without_a_build(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            padding = self.fixture(root)
            contracts.reject_unmatched_controls(root, self.cases)
            (root / "src/lib.rs").write_text(padding + padding)
            with self.assertRaisesRegex(ValueError, "no longer matches: first, second"):
                contracts.reject_unmatched_controls(root, self.cases)
            self.assertEqual(self.targets(root), {"first", "second"})

    def run_gate(self, root, changed_only=True, fresh=False):
        tested, controlled = [], []
        def run_tests(_root, pending, _invoke, stamps, _evidence, record, **_options):
            for case in pending:
                tested.append(case["id"])
                record({"id": case["id"], "status": "passed", "fingerprint": stamps[case["id"]]})
        def run_controls(_root, pending, stamps, _evidence, record):
            for case in pending:
                controlled.append(case["id"])
                record({"id": case["id"], "status": "passed", "fingerprint": stamps[case["id"]]})
        with patch.object(contracts, "run_tests", side_effect=run_tests), \
             patch.object(contracts, "negative_controls", side_effect=run_controls), \
             patch.object(contracts, "contract_fingerprints", side_effect=lambda _root, _policy, cases, _tools, phase="complete":
                          {case["id"]: f"{self.generation}:{phase}" for case in cases}), \
             patch.object(contracts.verify, "tool_identity", return_value={}), redirect_stdout(io.StringIO()):
            code = contracts.run(root, self.cases, policy=self.policy, changed_only=changed_only, fresh=fresh)
        return code, tested, controlled

    def report(self, root):
        return json.loads((root / "target/verification/product-contracts.json").read_text())

    def test_a_task_runs_changed_controls_and_the_full_tier_runs_what_a_linked_change_expired(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            padding = self.fixture(root)
            self.generation = 1
            self.assertEqual(self.run_gate(root, changed_only=False), (0, ["first", "second"], ["first", "second"]))
            # A linked dependency changes: every test reruns, no site changed, no control runs.
            self.generation = 2
            self.assertEqual(self.run_gate(root), (0, ["first", "second"], []))
            report = self.report(root)
            self.assertEqual((report["status"], report["execution"]["control_scope"], report["execution"]["negative_pending"]),
                             ("passed", "changed", 0))
            self.assertEqual({item["fingerprint"] for item in report["negative_controls"]}, {"1:complete"},
                             "an outdated control stays on record with its own identity")
            # The mutated text changes: its controls run once and then wait for another change to the site.
            self.generation = 3
            (root / "src/lib.rs").write_text(padding + "let beside = 1;\nlet defect = false;\n" + padding)
            self.assertEqual(self.run_gate(root), (0, ["first", "second"], ["first", "second"]))
            self.generation = 4
            self.assertEqual(self.run_gate(root), (0, ["first", "second"], []), "a repeated run must not repeat them")
            # The full tier accepts only the current identity, whatever the task tier did.
            self.assertEqual(self.run_gate(root, changed_only=False), (0, [], ["first", "second"]))
            self.assertEqual(self.run_gate(root, changed_only=False), (0, [], []))

    def test_a_failed_changed_control_fails_the_gate_and_is_not_kept(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            padding = self.fixture(root)
            self.generation = 1
            (root / "src/lib.rs").write_text(padding + "let beside = 1;\nlet defect = false;\n" + padding)
            def run_tests(_root, pending, _invoke, stamps, _evidence, record, **_options):
                for case in pending:
                    record({"id": case["id"], "status": "passed", "fingerprint": stamps[case["id"]]})
            def failing(_root, pending, stamps, _evidence, record):
                for case in pending:
                    record({"id": case["id"], "status": "failed", "fingerprint": stamps[case["id"]]})
            with patch.object(contracts, "run_tests", side_effect=run_tests), \
                 patch.object(contracts, "negative_controls", side_effect=failing), \
                 patch.object(contracts, "contract_fingerprints", return_value={"first": "x", "second": "x"}), \
                 patch.object(contracts.verify, "tool_identity", return_value={}), redirect_stderr(io.StringIO()):
                self.assertEqual(contracts.run(root, self.cases, policy=self.policy, changed_only=True), 1)
            self.assertEqual(self.report(root)["status"], "failed")
            self.assertEqual(self.run_gate(root)[2], ["first", "second"], "a failure is never reused as a pass")

    def test_every_real_counterexample_has_a_site_and_a_clean_tree_has_no_targets(self):
        policy = verify.load_policy(ROOT)
        with patch.object(contracts.verify, "changed_paths", return_value=[]), \
             patch.object(contracts, "read_at_base", side_effect=lambda root, path, base: (root / path).read_text()):
            sites, targets = contracts.control_sites(ROOT, policy, policy["product_contracts"])
        self.assertEqual(set(sites), {contract["id"] for contract in policy["product_contracts"]})
        self.assertEqual(targets, set())
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
        case = {"id": "a", "runner": "swift", "package_path": "swift/App", "build_product": "Tests", "suite": "S", "test": "a",
                "timeout": 180, "purpose": "fixture", "negative_control": {"file": "a", "old": "b", "new": "c"}}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            calls = []
            def built(args, **kwargs):
                calls.append(kwargs["timeout"])
                return subprocess.CompletedProcess(args, 0, "", "")
            with patch.object(contracts, "run_process", side_effect=built):
                contracts.build_swift_copies([root, root], [case])
            self.assertEqual(calls, [contracts.SWIFT_COPY_BUILD_TIMEOUT] * 2,
                             "bringing a copy up to date is not charged to a contract's deadline")
            def broken(args, **kwargs):
                return subprocess.CompletedProcess(args, 1, "", "error: no such module")
            with patch.object(contracts, "run_process", side_effect=broken), \
                 self.assertRaisesRegex(ValueError, "does not build:\nerror: no such module"):
                contracts.build_swift_copies([root], [case])


if __name__ == "__main__":
    unittest.main()
