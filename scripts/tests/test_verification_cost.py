"""Cost regressions must retain current linked inputs and assertion isolation."""

import hashlib
import importlib.util
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("cost_contracts", ROOT / "scripts/check_product_contracts.py")
contracts = importlib.util.module_from_spec(spec)
spec.loader.exec_module(contracts)
verify = contracts.verify


def hashed_alone(root, files, configuration, patterns, folders, tool_inputs, excludes):
    """The scope rule written out directly, with nothing shared between calls."""
    digest = hashlib.sha256(json.dumps(configuration, sort_keys=True).encode())
    for relative in sorted(set(files) | set(tool_inputs)):
        if (
            relative in tool_inputs
            or verify.matches([relative], patterns)
            or (
                any(relative.startswith(folder + "/") for folder in folders)
                and not verify.matches([relative], excludes)
            )
        ):
            digest.update(os.fsencode(relative) + b"\0" + verify.input_digest(root, relative))
    return digest.hexdigest()


class VerificationCostTests(unittest.TestCase):
    def test_a_shared_pass_hashes_each_scope_like_a_pass_of_its_own(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            files = ["one/lib.rs", "one/tests/audit.rs", "two/lib.rs", "readme.md"]
            for relative in files + ["tool-a", "tool-b"]:
                (root / relative).parent.mkdir(parents=True, exist_ok=True)
                (root / relative).write_text(relative)
            # Each pair differs in one part of the scope, so ignoring that part reuses the wrong files.
            scopes = [
                (["one/*"], (), (), ()),
                (["two/*"], (), (), ()),
                ([], ("one",), (), ()),
                ([], ("two",), (), ()),
                ([], ("one",), (), ("one/tests/*",)),
                ([], (), ("tool-a",), ()),
                ([], (), ("tool-b",), ()),
            ]
            shared = {}
            for scope in scopes * 2:
                self.assertEqual(
                    verify.scoped_digest(root, files, "configuration", *scope, scopes=shared),
                    hashed_alone(root, files, "configuration", *scope),
                    scope,
                )
            self.assertEqual(len(shared), len(scopes))
            self.assertNotEqual(
                verify.scoped_digest(root, files, "another", *scopes[0], scopes=shared),
                verify.scoped_digest(root, files, "configuration", *scopes[0], scopes=shared),
                "a shared scope must not hide a different configuration",
            )

    def test_each_fingerprint_pass_reads_a_shared_scope_once_and_the_next_pass_reads_it_again(self):
        policy = verify.load_policy(ROOT)
        cases = policy["product_contracts"]
        with patch.object(verify, "scope_entries", wraps=verify.scope_entries) as selection:
            contracts.contract_fingerprints(ROOT, policy, cases, {})
            first = selection.call_count
            contracts.contract_fingerprints(ROOT, policy, cases, {})
        self.assertGreater(first, 0, "a pass must read the files itself")
        self.assertLess(first, len(cases) / 4, "contracts sharing a scope must not reread its files")
        self.assertEqual(selection.call_count, 2 * first, "a later pass must not reuse an earlier read")

    def test_parsed_manifests_are_reused_only_while_the_file_is_unchanged(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "Cargo.toml"
            path.write_text('[package]\nname = "one"\n')
            first = verify.manifest(path)
            self.assertIs(verify.manifest(path), first)
            status = path.stat()
            # Same size and restored mtime: ctime still changes, so the edit is seen.
            path.write_text('[package]\nname = "two"\n')
            os.utime(path, ns=(status.st_atime_ns, status.st_mtime_ns))
            self.assertEqual(verify.manifest(path)["package"]["name"], "two")
            path.unlink()
            with self.assertRaises(FileNotFoundError):
                verify.manifest(path)

    def test_without_a_first_commit_the_index_is_the_change_and_a_bad_base_still_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            verify.git(root, "init", "--quiet")
            (root / "indexed.txt").write_text("indexed\n")
            (root / "untracked.txt").write_text("untracked\n")
            verify.git(root, "add", "indexed.txt")
            self.assertEqual(verify.changed_paths(root), ["indexed.txt", "untracked.txt"])
            with self.assertRaises(subprocess.CalledProcessError):
                verify.changed_paths(root, "no-such-ref")

    def test_diagnostic_edits_do_not_invalidate_product_evidence(self):
        policy = verify.load_policy(ROOT)
        cases = policy["product_contracts"]
        before = contracts.contract_fingerprints(ROOT, policy, cases, {})
        checks = verify.check_fingerprints(ROOT, policy, {}, {})
        digest = verify.input_digest
        for path in ("scripts/verification_metrics.py", "scripts/verification_display.py"):
            with (
                self.subTest(path=path),
                patch.object(
                    verify,
                    "input_digest",
                    side_effect=lambda root, relative, path=path: (
                        digest(root, relative) + (b"edited" if relative == path else b"")
                    ),
                ),
            ):
                self.assertEqual(
                    before,
                    contracts.contract_fingerprints(ROOT, policy, cases, {}),
                    "diagnostic code must not invalidate every contract",
                )
                after = verify.check_fingerprints(ROOT, policy, {}, {})
                # check-lint reads every Python file; it builds and tests no product.
                self.assertEqual(
                    {name for name in checks if checks[name] != after[name]}, {"check-verification", "check-lint"}
                )

    def test_ci_and_diagnostics_select_foundation_not_unrelated_sanitizers(self):
        policy = verify.load_policy(ROOT)
        always = {check["id"] for check in policy["checks"] if check.get("always")}
        for path in (
            ".github/workflows/check.yml",
            "scripts/wait_for_verify.py",
            "scripts/verification_metrics.py",
            "scripts/verification_display.py",
        ):
            self.assertEqual(
                set(verify.select_checks(policy, [path], redefined=set())),
                always,
                "adapters and diagnostics must not select all product checks",
            )

    def test_rust_identical_mutations_share_source_transition_not_test_processes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "lib.rs"
            source.write_text("let a = 1;\nlet b = 1;\n")
            fault = {"file": "lib.rs", "old": "a = 1", "new": "a = 2"}
            cases = [
                {
                    "id": name,
                    "runner": "rust",
                    "package": "fixture",
                    "target": ["--lib"],
                    "test": name,
                    "timeout": 10,
                    "negative_control": fault,
                }
                for name in ("one", "two")
            ]
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

            with (
                patch.object(contracts, "run_process", side_effect=invoke),
                patch.object(contracts, "mutated", side_effect=mutation),
            ):
                contracts.rust_controls(root, cases, {}, root / "cargo", root, results.append)
            self.assertEqual(len(transitions), 2, "identical Rust counterexamples must not restore/recompile twice")
            self.assertEqual([name for name, _ in states], ["one", "two", "two"])
            self.assertEqual([result["status"] for result in results], ["passed", "passed"])
            self.assertIn("a = 1", states[-1][1])
            self.assertEqual(source.read_text(), "let a = 1;\nlet b = 1;\n")

    def test_tsan_product_and_probe_share_only_the_instrumented_target(self):
        source = (ROOT / "scripts/check-product-bridge.sh").read_text()
        self.assertIn(
            'export CARGO_TARGET_DIR="$PWD/target/bridge/thread/rust"',
            source,
            "same TSan configuration must reuse build-std and dependencies",
        )
        self.assertIn("-Zbuild-std", source)
        self.assertIn("-Zsanitizer=thread -Zexternal-clangrt", source)

    def test_direct_swift_runner_keeps_filter_environment_and_one_test_gate(self):
        import swift_contract_runner as runner

        case = {
            "id": "a-contract",
            "runner": "swift",
            "package_path": "App",
            "build_product": "EchodeAppTests",
            "suite": "Suite",
            "test": "contract",
        }
        with patch.object(
            runner, "runtime", return_value=("/tool/helper", "/build/tests", "/sdk/frameworks", "/sdk/lib")
        ):
            args, env = runner.invocation(ROOT, case, "scratch")
        self.assertEqual(
            args,
            [
                "/tool/helper",
                "--test-bundle-path",
                "/build/tests",
                "--filter",
                "Suite/contract",
                # The stream the watchdog reads; each contract has its own (ADR 0325).
                "--event-stream-output-path",
                str(ROOT / "scratch/contract-events/a-contract.events"),
                "--event-stream-version",
                "0",
                "/build/tests",
                "--testing-library",
                "swift-testing",
            ],
        )
        self.assertTrue(env["DYLD_FRAMEWORK_PATH"].startswith("/sdk/frameworks"))
        self.assertTrue(env["DYLD_LIBRARY_PATH"].startswith("/sdk/lib"))
        for code, output in (
            (0, "Test run with 0 tests passed"),
            (0, "Test contract() passed\nTest run with 2 tests passed"),
            (0, "Test contract() passed\nTest run with 1 test passed\nknown issue"),
        ):
            self.assertFalse(contracts.passed(case, code, output))
        self.assertFalse(contracts.rejected_behavior(case, 69, "runner failed to load"))
        self.assertTrue(
            contracts.rejected_behavior(
                case, 1, "Expectation failed\nTest contract() failed\nTest run with 1 test in 1 suite failed"
            )
        )

    def test_rust_shared_mutation_failure_or_interruption_never_passes_partial_contract(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "lib.rs"
            source.write_text("let a = 1; let b = 1;")
            case = {
                "id": "case",
                "runner": "rust",
                "package": "fixture",
                "target": ["--lib"],
                "test": "t",
                "timeout": 10,
                "negative_controls": [
                    {"file": "lib.rs", "old": "a = 1", "new": "a = 2"},
                    {"file": "lib.rs", "old": "b = 1", "new": "b = 2"},
                ],
            }
            failed = subprocess.CompletedProcess([], 101, "error: could not compile", "")
            results = []
            with (
                patch.object(contracts, "run_process", side_effect=[failed, KeyboardInterrupt]),
                self.assertRaises(KeyboardInterrupt),
            ):
                contracts.rust_controls(root, [case], {}, root / "cargo", root, results.append)
            self.assertEqual(results, [])
            self.assertEqual(source.read_text(), "let a = 1; let b = 1;")


if __name__ == "__main__":
    unittest.main()
