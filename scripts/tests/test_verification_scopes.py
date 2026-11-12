"""Cache boundaries must follow compiled targets, not whole dependency folders."""

import importlib.util
import io
import json
import re
import subprocess
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("scope_contracts", ROOT / "scripts/check_product_contracts.py")
contracts = importlib.util.module_from_spec(spec)
spec.loader.exec_module(contracts)
verify = contracts.verify


def actual_policy_impacts():
    """Project byte edits through real fingerprints without editing sources or evidence."""
    policy = verify.load_policy(ROOT)
    cases = policy["product_contracts"]
    before = contracts.contract_fingerprints(ROOT, policy, cases, {})
    digest = verify.input_digest
    scenarios = {
        "history-integration-test": {"crates/echode-history/tests/float_roundtrip.rs"},
        "plugin-audit-and-fixture": {
            "crates/echode-plugin-host/tests/audit.rs",
            "crates/echode-test-plugin/src/lib.rs",
        },
        "history-library": {"crates/echode-history/src/media.rs"},
    }
    impacts = {}
    for label, edits in scenarios.items():

        def edited_digest(root, relative):
            return digest(root, relative) + (b"synthetic-byte-edit" if relative in edits else b"")

        with patch.object(verify, "input_digest", side_effect=edited_digest):
            after = contracts.contract_fingerprints(ROOT, policy, cases, {})
        affected = [case for case in cases if before[case["id"]] != after[case["id"]]]
        impacts[label] = affected
    return impacts


class VerificationScopeTests(unittest.TestCase):
    def fixture(self, root):
        subprocess.run(["git", "init", "-q"], cwd=root, check=True)
        (root / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/*"]\n')
        for name, dependency in (
            ("echode-model", None),
            ("echode-history", "echode-model"),
            ("echode-engine", "echode-model"),
            ("echode-sine-pad", "echode-engine"),
            ("echode-test-plugin", None),
            ("echode-plugin-host", "echode-engine"),
            ("echode-ffi", "echode-history"),
            ("echode-api", "echode-history"),
            ("echode-cli", "echode-history"),
        ):
            folder = root / "crates" / name
            (folder / "src").mkdir(parents=True)
            (folder / "tests").mkdir()
            dependency = f'[dependencies]\n{dependency} = {{ path = "../{dependency}" }}\n' if dependency else ""
            (folder / "Cargo.toml").write_text(f'[package]\nname = "{name}"\nversion = "0.1.0"\n{dependency}')
            (folder / "src/lib.rs").write_text("// original\n")
            (folder / "tests/audit.rs").write_text("// ignored audit fixture\n")
        (root / "swift/EchodeApp").mkdir(parents=True)
        (root / "swift/EchodeApp/View.swift").write_text("// original\n")
        policy = json.loads((ROOT / "scripts/verification.json").read_text())
        cases = [
            {
                "id": name,
                "runner": "rust",
                "package": name,
                "target": ["--lib"],
                "test": "t",
                "timeout": 5,
                "negative_control": {"file": f"crates/{name}/src/lib.rs", "old": "original", "new": "broken"},
            }
            for name in ("echode-api", "echode-history", "echode-engine")
        ]
        cases.append(
            {
                "id": "swift",
                "runner": "swift",
                "package_path": "swift/EchodeApp",
                "test": "t",
                "suite": "S",
                "timeout": 5,
                "negative_control": {"file": "swift/EchodeApp/View.swift", "old": "original", "new": "broken"},
            }
        )
        return policy, cases

    def test_actual_policy_preserves_linked_code_and_does_not_hash_unlinked_tests(self):
        impacts = actual_policy_impacts()
        self.assertTrue(impacts["history-integration-test"])
        self.assertTrue(all(case.get("package") == "echode-history" for case in impacts["history-integration-test"]))
        self.assertFalse(
            any(
                case.get("package") in ("echode-api", "echode-history", "echode-model", "echode-draw")
                for case in impacts["plugin-audit-and-fixture"]
            )
        )
        self.assertTrue(
            any(case["runner"] == "swift" for case in impacts["history-library"]),
            "real backend changes must still invalidate Swift counterexamples",
        )

    def test_every_registered_mutation_is_inside_the_effective_hashed_scope(self):
        policy = verify.load_policy(ROOT)
        for case in policy["product_contracts"]:
            patterns, folders = contracts.contract_inputs(ROOT, policy, case)
            tested = (
                {verify.workspace_members(ROOT)[case["package"]].relative_to(ROOT).as_posix()}
                if case["runner"] == "rust"
                else set()
            )
            excludes = verify.package_test_excludes(folders, tested, ROOT)
            for fault in contracts.negative_control_mutations(case):
                path = fault["file"]
                self.assertTrue(verify.matches([path], patterns) or not verify.matches([path], excludes), case["id"])

    def test_test_plugin_is_not_an_input_to_history_or_rpc(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            policy, cases = self.fixture(root)
            before = contracts.contract_fingerprints(root, policy, cases, {})
            (root / "crates/echode-test-plugin/src/lib.rs").write_text("// changed\n")
            after = contracts.contract_fingerprints(root, policy, cases, {})
            changed = {name for name in before if before[name] != after[name]}
            self.assertEqual(
                changed, {"echode-engine", "swift"}, "plugin fixtures must not contaminate unrelated contracts"
            )

    def test_dependency_integration_tests_do_not_change_linked_contracts(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            policy, cases = self.fixture(root)
            before = contracts.contract_fingerprints(root, policy, cases, {})
            (root / "crates/echode-history/tests/audit.rs").write_text("// formatted\n")
            after = contracts.contract_fingerprints(root, policy, cases, {})
            self.assertEqual(
                {name for name in before if before[name] != after[name]},
                {"echode-history"},
                "integration tests are not linked into dependants",
            )
            before = after
            (root / "crates/echode-history/src/lib.rs").write_text("// changed\n")
            after = contracts.contract_fingerprints(root, policy, cases, {})
            self.assertEqual(
                {name for name in before if before[name] != after[name]},
                {"echode-api", "echode-history", "swift"},
                "linked backend changes still invalidate Swift",
            )

    def test_each_phase_resumes_independently_and_force_still_runs_everything(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / ".gitignore").write_text("/target\n")
            policy, cases = self.fixture(root)
            normal, negative = [], []

            def run_tests(_root, pending, _invoke, stamps, _evidence, record, **_options):
                for case in pending:
                    normal.append(case["id"])
                    record({"id": case["id"], "status": "passed", "fingerprint": stamps[case["id"]]})

            def run_controls(_root, pending, stamps, _evidence, record):
                for case in pending:
                    negative.append(case["id"])
                    record({"id": case["id"], "status": "passed", "fingerprint": stamps[case["id"]]})

            with (
                patch.object(contracts.verify, "tool_identity", return_value={}),
                patch.object(contracts, "run_tests", side_effect=run_tests),
                patch.object(contracts, "negative_controls", side_effect=run_controls),
                redirect_stdout(io.StringIO()),
            ):
                self.assertEqual(contracts.run(root, cases, policy=policy), 0)
                normal.clear()
                negative.clear()
                cases[0]["negative_control"]["new"] = "different defect"
                self.assertEqual(contracts.run(root, cases, policy=policy), 0)
                self.assertEqual(normal, [], "changing a mutation must not rerun the unchanged positive test")
                self.assertEqual(negative, ["echode-api"])
                report_path = root / "target/verification/product-contracts.json"
                report = json.loads(report_path.read_text())
                self.assertEqual(report["execution"]["normal_pending"], 0)
                self.assertEqual(report["execution"]["negative_pending"], 1)
                # A partial/failed control is not reusable, even with a valid normal pass.
                report["negative_controls"][0]["status"] = "failed"
                report_path.write_text(json.dumps(report))
                negative.clear()
                self.assertEqual(contracts.run(root, cases, policy=policy), 0)
                self.assertEqual(normal, [])
                self.assertEqual(negative, ["echode-api"])
                normal.clear()
                negative.clear()
                self.assertEqual(contracts.run(root, cases, policy=policy, fresh=True), 0)
                self.assertEqual(normal, [case["id"] for case in cases])
                self.assertEqual(negative, normal)

    def test_gate_errors_keep_finished_phase_evidence_for_a_safe_retry(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            report = root / "target/verification/product-contracts.json"
            report.parent.mkdir(parents=True)
            finished = {"id": "case", "status": "passed", "fingerprint": "current-inputs"}
            report.write_text(json.dumps({"status": "running", "tests": [finished], "negative_controls": []}))
            with (
                patch.object(contracts, "ROOT", root),
                patch.object(contracts.verify, "load_policy", return_value={"product_contracts": []}),
                patch.object(contracts, "run", side_effect=ValueError("negative control no longer matches")),
                patch("sys.stderr", io.StringIO()),
            ):
                self.assertEqual(contracts.main(), 1)
            saved = json.loads(report.read_text())
            self.assertEqual(saved["status"], "failed")
            self.assertEqual(saved["tests"], [finished])
            self.assertEqual(saved["negative_controls"], [])

    def test_mutation_definitions_do_not_invalidate_normal_test_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            policy, cases = self.fixture(root)
            normal = contracts.contract_fingerprints(root, policy, cases, {}, "tests")
            negative = contracts.contract_fingerprints(root, policy, cases, {})
            cases[0]["negative_control"]["new"] = "a different defect"
            self.assertEqual(normal, contracts.contract_fingerprints(root, policy, cases, {}, "tests"))
            after = contracts.contract_fingerprints(root, policy, cases, {})
            self.assertEqual({name for name in negative if negative[name] != after[name]}, {"echode-api"})

    def test_custom_targets_and_build_scripts_keep_package_test_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _, _cases = self.fixture(root)
            folder = "crates/echode-history"
            self.assertIn(folder + "/tests/**", verify.package_test_excludes({folder}, root=root))
            build = root / folder / "build.rs"
            build.write_text("// reads arbitrary package inputs\n")
            self.assertEqual(verify.package_test_excludes({folder}, root=root), [])
            build.unlink()
            manifest = root / folder / "Cargo.toml"
            manifest.write_text(manifest.read_text() + '\n[lib]\npath = "tests/library.rs"\n')
            self.assertEqual(verify.package_test_excludes({folder}, root=root), [])

    def test_hook_budgets_cover_serial_checks_and_lock_wait(self):
        policy = verify.load_policy(ROOT)
        budget = policy["hook_timeout"]
        import wait_for_verify

        self.assertEqual(wait_for_verify.DEFAULT_TIMEOUT, budget - 10)
        automatic = [check for check in policy["checks"] if check["mode"] == "automatic"]
        preflights = {name for check in automatic for name in check.get("preflight", [])}
        required = sum(check["timeout"] for check in automatic) + sum(
            policy["preflights"][name]["timeout"] for name in preflights
        )
        self.assertGreaterEqual(budget, required + 120)
        cursor = json.loads((ROOT / ".cursor/hooks.json").read_text())["hooks"]
        codex = json.loads((ROOT / ".codex/hooks.json").read_text())["hooks"]
        for entries in cursor.values():
            for entry in entries:
                self.assertEqual(entry["timeout"], budget)
        for entries in codex.values():
            for entry in entries:
                for hook in entry["hooks"]:
                    self.assertEqual(hook["timeout"], budget)

    def test_policy_rejects_a_hook_budget_shorter_than_serial_verification(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            policy = verify.load_policy(ROOT)
            policy["hook_timeout"] = 3600
            (root / "scripts").mkdir()
            (root / verify.POLICY).write_text(json.dumps(policy))
            with self.assertRaisesRegex(ValueError, "hook budget must cover"):
                verify.load_policy(root)

    def test_generated_header_copy_is_content_stable_and_still_updates_changes(self):
        for relative in ("scripts/check-shell.sh", "scripts/check-product-bridge.sh", "Makefile"):
            text = (ROOT / relative).read_text().replace("\\\n", " ").replace("$$", "$")
            commands = re.findall(r"cmp -s [^;\n]+? \|\|\s*cp [^;\n]+", text)
            self.assertTrue(commands, relative)
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                header = root / "generated.h"
                header.write_text("first\n")
                include = root / "swift/EchodeBridge/CEchodeFfi/include"
                include.mkdir(parents=True)
                destination = include / "echode.h"
                command = commands[0].strip()
                env = {
                    "PATH": "/usr/bin:/bin",
                    "header": str(header),
                    "product_header": str(header),
                    "include": str(include),
                }

                def copy_header():
                    subprocess.run(["bash", "-euo", "pipefail", "-c", command], cwd=root, env=env, check=True)

                copy_header()
                before = destination.stat().st_mtime_ns
                copy_header()
                self.assertEqual(destination.stat().st_mtime_ns, before, relative)
                header.write_text("changed\n")
                copy_header()
                self.assertEqual(destination.read_text(), "changed\n", relative)

    def test_library_bridge_scope_ignores_downstream_integration_tests_but_not_sources(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            policy, _cases = self.fixture(root)
            policy["checks"] = [
                {
                    "id": "bridge",
                    "mode": "automatic",
                    "areas": [],
                    "timeout": 5,
                    "packages": ["echode-ffi"],
                    "package_scope": "targets",
                    "inputs": [],
                }
            ]
            policy["preflights"] = {}
            initial = verify.check_fingerprints(root, policy, {}, {})
            integration = root / "crates/echode-history/tests/audit.rs"
            integration.write_text("// formatted\n")
            self.assertEqual(initial, verify.check_fingerprints(root, policy, {}, {}))
            integration.unlink()
            self.assertEqual(initial, verify.check_fingerprints(root, policy, {}, {}))
            source = root / "crates/echode-history/src/lib.rs"
            source.write_text("// changed\n")
            self.assertNotEqual(initial, verify.check_fingerprints(root, policy, {}, {}))
            # A check may explicitly run a fixture from a dependency's tests directory.
            integration.write_text("// selected fixture\n")
            policy["checks"][0]["inputs"] = ["crates/echode-history/tests/**"]
            before = verify.check_fingerprints(root, policy, {}, {})
            integration.write_text("// changed fixture\n")
            self.assertNotEqual(before, verify.check_fingerprints(root, policy, {}, {}))


if __name__ == "__main__":
    unittest.main()
