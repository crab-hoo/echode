"""Cost regressions must retain current linked inputs and assertion isolation."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("cost_contracts", ROOT / "scripts/check_product_contracts.py")
contracts = importlib.util.module_from_spec(spec)
spec.loader.exec_module(contracts)
verify = contracts.verify


class VerificationCostTests(unittest.TestCase):
    def test_diagnostic_edits_do_not_invalidate_product_evidence(self):
        policy = verify.load_policy(ROOT)
        cases = policy["product_contracts"]
        before = contracts.contract_fingerprints(ROOT, policy, cases, {})
        checks = verify.check_fingerprints(ROOT, policy, {}, {})
        digest = verify.input_digest
        for path in ("scripts/verification_metrics.py", "scripts/verification_display.py"):
            with self.subTest(path=path), patch.object(verify, "input_digest", side_effect=lambda root, relative:
                    digest(root, relative) + (b"edited" if relative == path else b"")):
                self.assertEqual(before, contracts.contract_fingerprints(ROOT, policy, cases, {}),
                                 "diagnostic code must not invalidate every contract")
                after = verify.check_fingerprints(ROOT, policy, {}, {})
                self.assertEqual({name for name in checks if checks[name] != after[name]}, {"check-verification"})

    def test_hooks_and_ci_select_foundation_not_unrelated_sanitizers(self):
        policy = verify.load_policy(ROOT)
        always = {check["id"] for check in policy["checks"] if check.get("always")}
        for path in (".cursor/hooks.json", ".codex/hooks.json", ".github/workflows/check.yml",
                     "scripts/wait_for_verify.py", "scripts/verification_metrics.py", "scripts/verification_display.py"):
            self.assertEqual(set(verify.select_checks(policy, [path], redefined=set())), always,
                             "adapters and diagnostics must not select all product checks")

    def test_rust_identical_mutations_share_source_transition_not_test_processes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "lib.rs"
            source.write_text("let a = 1;\nlet b = 1;\n")
            fault = {"file": "lib.rs", "old": "a = 1", "new": "a = 2"}
            cases = [{"id": name, "runner": "rust", "package": "fixture", "target": ["--lib"],
                      "test": name, "timeout": 10, "negative_control": fault} for name in ("one", "two")]
            cases[1]["negative_controls"] = [fault, {"file": "lib.rs", "old": "b = 1", "new": "b = 2"}]
            states, results = [], []
            original = contracts.mutated
            transitions = []
            def mutation(*args):
                transitions.append(args[2])
                return original(*args)
            def invoke(args, **kwargs):
                name = args[args.index("--") - 1]
                states.append((name, source.read_text()))
                output = f"test {name} ... FAILED\ntest result: FAILED. 0 passed; 1 failed; 0 ignored;"
                return subprocess.CompletedProcess(args, 101, output, "")
            with patch.object(contracts, "run_process", side_effect=invoke), patch.object(contracts, "mutated", side_effect=mutation):
                contracts.rust_controls(root, cases, {}, root / "cargo", root, results.append)
            self.assertEqual(len(transitions), 2, "identical Rust counterexamples must not restore/recompile twice")
            self.assertEqual([name for name, _ in states], ["one", "two", "two"])
            self.assertEqual([result["status"] for result in results], ["passed", "passed"])
            self.assertIn("a = 1", states[-1][1])
            self.assertEqual(source.read_text(), "let a = 1;\nlet b = 1;\n")

    def test_tsan_product_and_probe_share_only_the_instrumented_target(self):
        source = (ROOT / "scripts/check-product-bridge.sh").read_text()
        self.assertIn('export CARGO_TARGET_DIR="$PWD/target/bridge/thread/rust"', source,
                      "same TSan configuration must reuse build-std and dependencies")
        self.assertIn("-Zbuild-std", source)
        self.assertIn("-Zsanitizer=thread -Zexternal-clangrt", source)

    def test_direct_swift_runner_keeps_filter_environment_and_one_test_gate(self):
        import swift_contract_runner as runner
        case = {"runner": "swift", "package_path": "App", "build_product": "EchodeAppTests",
                "suite": "Suite", "test": "contract"}
        with patch.object(runner, "runtime", return_value=("/tool/helper", "/build/tests", "/sdk/frameworks", "/sdk/lib")):
            args, env = runner.invocation(ROOT, case, "scratch")
        self.assertEqual(args, ["/tool/helper", "--test-bundle-path", "/build/tests", "--filter", "Suite/contract",
                                "/build/tests", "--testing-library", "swift-testing"])
        self.assertTrue(env["DYLD_FRAMEWORK_PATH"].startswith("/sdk/frameworks"))
        self.assertTrue(env["DYLD_LIBRARY_PATH"].startswith("/sdk/lib"))
        for code, output in ((0, "Test run with 0 tests passed"),
                             (0, "Test contract() passed\nTest run with 2 tests passed"),
                             (0, "Test contract() passed\nTest run with 1 test passed\nknown issue")):
            self.assertFalse(contracts.passed(case, code, output))
        self.assertFalse(contracts.rejected_behavior(case, 69, "runner failed to load"))
        self.assertTrue(contracts.rejected_behavior(case, 1,
                        "Expectation failed\nTest contract() failed\nTest run with 1 test in 1 suite failed"))

    def test_rust_shared_mutation_failure_or_interruption_never_passes_partial_contract(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "lib.rs"
            source.write_text("let a = 1; let b = 1;")
            case = {"id": "case", "runner": "rust", "package": "fixture", "target": ["--lib"],
                    "test": "t", "timeout": 10, "negative_controls": [
                        {"file": "lib.rs", "old": "a = 1", "new": "a = 2"},
                        {"file": "lib.rs", "old": "b = 1", "new": "b = 2"}]}
            failed = subprocess.CompletedProcess([], 101, "error: could not compile", "")
            results = []
            with patch.object(contracts, "run_process", side_effect=[failed, KeyboardInterrupt]), self.assertRaises(KeyboardInterrupt):
                contracts.rust_controls(root, [case], {}, root / "cargo", root, results.append)
            self.assertEqual(results, [])
            self.assertEqual(source.read_text(), "let a = 1; let b = 1;")


if __name__ == "__main__":
    unittest.main()
