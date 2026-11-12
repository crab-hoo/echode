"""A green process exit must also prove the required regression ran."""

import errno
import importlib.util
import io
import json
import os
import subprocess
import tempfile
import threading
import time
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("contracts", ROOT / "scripts/check_product_contracts.py")
contracts = importlib.util.module_from_spec(spec)
spec.loader.exec_module(contracts)


def git_repository(root):
    # Untracked, non-ignored files are sources too; the fixture needs no commit.
    subprocess.run(["git", "init", "--quiet"], cwd=root, check=True)
    (root / ".gitignore").write_text("/target\n")


def artifact(folder, kind, name, executable):
    """One `cargo test --message-format=json` line for a test binary."""
    return {
        "reason": "compiler-artifact",
        "manifest_path": str(folder / "Cargo.toml"),
        "target": {"kind": kind, "name": name},
        "profile": {"test": True},
        "executable": executable,
    }


class ContractEvidenceTests(unittest.TestCase):
    def test_removal_retries_metadata_created_during_cleanup(self):
        remove = contracts.shutil.rmtree
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory) / "copy"
            folder.mkdir()

            def metadata_race(path):
                if not (path / ".DS_Store").exists():
                    (path / ".DS_Store").write_bytes(b"metadata")
                    raise OSError(errno.ENOTEMPTY, "Directory not empty", str(path))
                remove(path)

            with patch.object(contracts.shutil, "rmtree", side_effect=metadata_race) as cleanup:
                contracts.remove_path(folder)
            self.assertEqual(cleanup.call_count, 2)
            self.assertFalse(folder.exists())

    def test_removal_does_not_hide_cleanup_failures(self):
        for code, attempts in ((errno.ENOTEMPTY, 3), (errno.EACCES, 1)):
            with self.subTest(errno=code), tempfile.TemporaryDirectory() as directory:
                folder = Path(directory) / "copy"
                folder.mkdir()
                error = OSError(code, "cleanup failed")
                with patch.object(contracts.shutil, "rmtree", side_effect=error) as cleanup:
                    with self.assertRaises(OSError) as raised:
                        contracts.remove_path(folder)
                    self.assertIs(raised.exception, error)
                    self.assertEqual(cleanup.call_count, attempts)

    def test_persistent_copy_keeps_unchanged_times_and_renews_every_changed_file(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "repo"
            (root / "src/nested").mkdir(parents=True)
            git_repository(root)
            (root / "src/a.rs").write_text("a\n")
            (root / "src/b.swift").write_text("b\n")
            (root / "src/nested/c.txt").write_text("c\n")
            (root / "src/link.rs").symlink_to("a.rs")
            (root / contracts.HEADER).parent.mkdir(parents=True)
            (root / contracts.HEADER).write_text("header\n")
            (root / "target/debug").mkdir(parents=True)
            checkout = Path(directory) / "checkouts/0"
            contracts.sync_checkout(root, checkout)
            self.assertEqual((checkout / "src/a.rs").read_text(), "a\n")
            self.assertEqual(os.readlink(checkout / "src/link.rs"), "a.rs")
            self.assertEqual((checkout / contracts.HEADER).read_text(), "header\n")
            self.assertEqual(Path(os.readlink(checkout / "target/debug")), root / "target/debug")
            for name in ("src/a.rs", "src/b.swift"):
                os.utime(checkout / name, ns=(10**18, 10**18))
            # An edit carrying an older time (a restore, cp -p, an archive) must
            # still look newer than any build made from the previous contents.
            (root / "src/a.rs").write_text("edited\n")
            os.utime(root / "src/a.rs", ns=(10**9, 10**9))
            (root / "src/nested/c.txt").unlink()
            (checkout / "target/scratch").mkdir()
            contracts.sync_checkout(root, checkout)
            self.assertEqual((checkout / "src/a.rs").read_text(), "edited\n")
            self.assertGreater((checkout / "src/a.rs").stat().st_mtime_ns, 10**18)
            self.assertEqual((checkout / "src/b.swift").stat().st_mtime_ns, 10**18)
            self.assertFalse((checkout / "src/nested").exists())
            self.assertTrue((checkout / "target/scratch").is_dir())
            # A counterexample left behind by an interrupted run is replaced.
            (checkout / "src/b.swift").write_text("mutated\n")
            contracts.sync_checkout(root, checkout)
            self.assertEqual((checkout / "src/b.swift").read_text(), "b\n")
            # Unknown contents could hold stale sources, so they are removed; builds stay.
            (checkout / "src/stray.swift").write_text("stale\n")
            checkout.with_name("0.json").write_text("not json")
            contracts.sync_checkout(root, checkout)
            self.assertFalse((checkout / "src/stray.swift").exists())
            self.assertTrue((checkout / "target/scratch").is_dir())
            self.assertEqual((checkout / "src/b.swift").read_text(), "b\n")

    def test_a_restored_counterexample_is_newer_than_its_build(self):
        with tempfile.TemporaryDirectory() as directory:
            checkout = Path(directory)
            (checkout / "src").mkdir()
            source = checkout / "src/lib.rs"
            source.write_text("fn value() -> u8 { 1 }\n")
            contract = {"id": "fixture", "negative_control": {"file": "src/lib.rs", "old": "{ 1 }", "new": "{ 2 }"}}
            with contracts.mutated(checkout, contract, contract["negative_control"]):
                self.assertIn("{ 2 }", source.read_text())
                built = time.time_ns()
                os.utime(source, ns=(built - 10**9, built - 10**9))
            self.assertEqual(source.read_text(), "fn value() -> u8 { 1 }\n")
            self.assertGreater(source.stat().st_mtime_ns, built - 10**9)
            stale = dict(contract, negative_control={"file": "src/lib.rs", "old": "missing", "new": "x"})
            with self.assertRaises(ValueError), contracts.mutated(checkout, stale, stale["negative_control"]):
                pass

    def test_rust_counterexamples_change_dependencies_first_and_group_each_crate(self):
        def case(name, path, package="echode-ffi"):
            return {
                "id": name,
                "runner": "rust",
                "package": package,
                "target": ["--lib"],
                "negative_control": {"file": path},
            }

        ordered = contracts.mutation_order(
            ROOT,
            [
                case("ffi", "crates/echode-ffi/src/snapshot.rs"),
                case("model", "crates/echode-model/src/lib.rs", "echode-history"),
                case("engine-a", "crates/echode-engine/src/lib.rs", "echode-engine"),
                case("model-again", "crates/echode-model/src/lib.rs", "echode-history"),
                case("engine-b", "crates/echode-engine/src/lib.rs", "echode-engine"),
            ],
        )
        self.assertEqual([item["id"] for item in ordered], ["model", "model-again", "engine-a", "engine-b", "ffi"])

    def test_swift_counterexamples_build_together_and_test_alone(self):
        with tempfile.TemporaryDirectory() as directory:
            checkouts = [Path(directory) / str(index) for index in range(3)]
            for checkout in checkouts:
                (checkout / "Sources").mkdir(parents=True)
                (checkout / "Sources/View.swift").write_text("let a = 1\nlet b = 1\nlet c = 1\nlet d = 1\n")
            cases = [
                {
                    "id": f"case-{name}",
                    "runner": "swift",
                    "package_path": "App",
                    "suite": "Suite",
                    "test": f"contract{name}",
                    "timeout": 30,
                    "negative_control": {
                        "file": "Sources/View.swift",
                        "old": f"let {name} = 1",
                        "new": f"let {name} = 2",
                    },
                }
                for name in "abcd"
            ]
            lock = threading.Lock()
            state = {"building": 0, "most": 0, "tests": []}

            def fake(args, *, cwd, capture_output, text, timeout, env=None):
                if args[:2] == ["swift", "build"]:
                    with lock:
                        state["building"] += 1
                        state["most"] = max(state["most"], state["building"])
                    time.sleep(0.05)
                    with lock:
                        state["building"] -= 1
                    return subprocess.CompletedProcess(args, 0, "Build complete!", "")
                self.assertIn("--skip-build", args)
                self.assertEqual(state["building"], 0)
                name = args[-1].split("/")[1]
                self.assertIn(name[-1] + " = 2", (Path(cwd) / "Sources/View.swift").read_text())
                state["tests"].append(name)
                output = f"Expectation failed\nTest {name}() failed\nTest run with 1 test in 1 suite failed"
                return subprocess.CompletedProcess(args, 1, output, "")

            results = []
            with patch.object(contracts, "run_process", side_effect=fake):
                contracts.swift_controls(checkouts, cases, {}, Path(directory), results.append)
            self.assertEqual(state["most"], 3)
            self.assertEqual(state["tests"], ["contracta", "contractb", "contractc", "contractd"])
            self.assertEqual([result["status"] for result in results], ["passed"] * 4)
            for checkout in checkouts:
                self.assertEqual(
                    (checkout / "Sources/View.swift").read_text(), "let a = 1\nlet b = 1\nlet c = 1\nlet d = 1\n"
                )

    def test_interrupted_swift_controls_keep_completed_contracts_but_not_partial_mutations(self):
        with tempfile.TemporaryDirectory() as directory:
            checkout = Path(directory) / "0"
            (checkout / "Sources").mkdir(parents=True)
            source = checkout / "Sources/View.swift"
            original = "let a = 1\nlet b = 1\n"
            source.write_text(original)
            fault_a = {"file": "Sources/View.swift", "old": "let a = 1", "new": "let a = 2"}
            fault_b = {"file": "Sources/View.swift", "old": "let b = 1", "new": "let b = 2"}
            cases = [
                {
                    "id": name,
                    "runner": "swift",
                    "package_path": "App",
                    "suite": "Suite",
                    "test": f"contract{name}",
                    "timeout": 30,
                    "negative_controls": faults,
                }
                for name, faults in [("single", [fault_a]), ("multiple", [fault_a, fault_b])]
            ]
            fingerprints = {case["id"]: "stamp-" + case["id"] for case in cases}

            def fake(args, **kwargs):
                if args[:2] == ["swift", "build"]:
                    if "let b = 2" in source.read_text():
                        raise KeyboardInterrupt("interrupt during the next mutation")
                    return subprocess.CompletedProcess(args, 0, "Build complete!", "")
                name = args[-1].split("/")[1]
                output = f"Expectation failed\nTest {name}() failed\nTest run with 1 test in 1 suite failed"
                return subprocess.CompletedProcess(args, 1, output, "")

            results = []
            with patch.object(contracts, "run_process", side_effect=fake), self.assertRaises(KeyboardInterrupt):
                contracts.swift_controls([checkout], cases, fingerprints, Path(directory), results.append)
            self.assertEqual(
                [result["id"] for result in results],
                ["single"],
                "a completed Swift contract must survive a later interruption",
            )
            self.assertEqual(results[0]["status"], "passed")
            self.assertIn("mutation 1/1: rejected", (Path(directory) / "single-negative.log").read_text())
            self.assertEqual(source.read_text(), original)
            tests = [{"id": name, "status": "passed", "fingerprint": stamp} for name, stamp in fingerprints.items()]
            reusable = contracts.reusable_results({"tests": tests, "negative_controls": results}, fingerprints)
            self.assertEqual(set(reusable), {"single"}, "a partial multi-mutation contract is never reusable")

    def test_multiple_counterexamples_are_each_built_and_all_must_be_rejected(self):
        def run(second_rejected):
            with tempfile.TemporaryDirectory() as directory:
                checkout = Path(directory) / "0"
                (checkout / "Sources").mkdir(parents=True)
                source = checkout / "Sources/View.swift"
                source.write_text("let a = 1\nlet b = 1\n")
                case = {
                    "id": "case",
                    "runner": "swift",
                    "package_path": "App",
                    "suite": "Suite",
                    "test": "contract",
                    "timeout": 30,
                    "negative_controls": [
                        {"file": "Sources/View.swift", "old": "let a = 1", "new": "let a = 2"},
                        {"file": "Sources/View.swift", "old": "let b = 1", "new": "let b = 2"},
                    ],
                }
                builds, tests = [], []

                def fake(args, **kwargs):
                    if args[:2] == ["swift", "build"]:
                        builds.append(source.read_text())
                        return subprocess.CompletedProcess(args, 0, "Build complete!", "")
                    text = source.read_text()
                    tests.append(text)
                    if not second_rejected and "let b = 2" in text:
                        return subprocess.CompletedProcess(
                            args, 0, "Test contract() passed\nTest run with 1 test in 1 suite passed", ""
                        )
                    output = "Expectation failed\nTest contract() failed\nTest run with 1 test in 1 suite failed"
                    return subprocess.CompletedProcess(args, 1, output, "")

                results = []
                with patch.object(contracts, "run_process", side_effect=fake):
                    contracts.swift_controls([checkout], [case], {}, Path(directory), results.append)
                self.assertEqual(len(builds), 2)
                self.assertEqual(len(tests), 2)
                self.assertIn("let a = 2", builds[0])
                self.assertIn("let b = 1", builds[0])
                self.assertIn("let b = 2", builds[1])
                self.assertIn("let a = 1", builds[1])
                self.assertEqual(source.read_text(), "let a = 1\nlet b = 1\n")
                return results[0]["status"]

        self.assertEqual(run(True), "passed")
        self.assertEqual(run(False), "failed")

    def test_a_counterexample_that_does_not_compile_is_not_detection(self):
        with tempfile.TemporaryDirectory() as directory:
            checkout = Path(directory) / "0"
            (checkout / "Sources").mkdir(parents=True)
            (checkout / "Sources/View.swift").write_text("let a = 1\n")
            case = {
                "id": "case",
                "runner": "swift",
                "package_path": "App",
                "suite": "Suite",
                "test": "contract",
                "timeout": 30,
                "negative_control": {"file": "Sources/View.swift", "old": "= 1", "new": "= ("},
            }
            calls = []

            def fake(args, **kwargs):
                calls.append(args)
                return subprocess.CompletedProcess(args, 1, "error: expected expression", "")

            results = []
            with (
                patch.object(contracts, "run_process", side_effect=fake),
                redirect_stdout(io.StringIO()),
                redirect_stderr(io.StringIO()),
            ):
                contracts.swift_controls([checkout], [case], {}, Path(directory), results.append)
            self.assertEqual(results[0]["status"], "failed")
            self.assertEqual(len(calls), 1)

    def test_swift_tests_share_one_build_but_each_runs_in_its_own_process(self):
        cases = [
            {
                "id": "rust-case",
                "runner": "rust",
                "package": "p",
                "target": ["--lib"],
                "test": "contract",
                "timeout": 5,
                "purpose": "fixture",
            }
        ]
        cases += [
            {
                "id": f"swift-{name}",
                "runner": "swift",
                "package_path": "App",
                "suite": "Suite",
                "test": f"contract{name}",
                "timeout": 5,
                "purpose": "fixture",
            }
            for name in "ab"
        ]

        def good(args, **kwargs):
            if args[:2] == ["swift", "build"]:
                return subprocess.CompletedProcess(args, 0, "Build complete!", "")
            if args[0] == "cargo":
                return subprocess.CompletedProcess(
                    args, 0, "test contract ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;", ""
                )
            name = args[-1].split("/")[1]
            return subprocess.CompletedProcess(
                args, 0, f"Test {name}() passed\nTest run with 1 test in 1 suite passed", ""
            )

        with tempfile.TemporaryDirectory() as directory:
            evidence = Path(directory)
            results, invoked = [], []

            def record(args, **kwargs):
                invoked.append(args)
                return good(args, **kwargs)

            contracts.run_tests(ROOT, cases, record, {}, evidence, results.append)
            self.assertEqual([result["status"] for result in results], ["passed"] * 3)
            self.assertEqual(sum(args[:2] == ["swift", "build"] for args in invoked), 1)
            swift_tests = [args for args in invoked if args[:2] == ["swift", "test"]]
            self.assertEqual(len(swift_tests), 2)
            for args in swift_tests:
                self.assertIn("--skip-build", args)
                self.assertEqual(args.count("--filter"), 1)
            results, invoked = [], []

            def broken(args, **kwargs):
                invoked.append(args)
                if args[:2] == ["swift", "build"]:
                    return subprocess.CompletedProcess(args, 1, "error: compile failure", "")
                return good(args, **kwargs)

            with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                contracts.run_tests(ROOT, cases, broken, {}, evidence, results.append)
            # Swift runs first (ADR 0328), so its failures come before the Rust pass.
            self.assertEqual([result["status"] for result in results], ["failed", "failed", "passed"])
            self.assertFalse([args for args in invoked if args[:2] == ["swift", "test"]])
            self.assertIn("compile failure", (evidence / "swift-a.log").read_text())

    def test_reuse_needs_a_pass_and_a_rejected_counterexample_for_the_same_inputs(self):
        test = {"id": "a", "status": "passed", "fingerprint": "one"}
        control = {"id": "a", "status": "passed", "fingerprint": "one"}
        self.assertIn("a", contracts.reusable_results({"tests": [test], "negative_controls": [control]}, {"a": "one"}))
        for tests, controls, stamps in (
            ([test], [control], {"a": "two"}),
            ([test], [], {"a": "one"}),
            ([test], [dict(control, status="failed")], {"a": "one"}),
            ([dict(test, status="failed")], [control], {"a": "one"}),
            ([test], [dict(control, fingerprint="two")], {"a": "one"}),
            ([dict(test, fingerprint=None)], [dict(control, fingerprint=None)], {"a": None}),
        ):
            with self.subTest(tests=tests, controls=controls, stamps=stamps):
                self.assertEqual(
                    contracts.reusable_results({"tests": tests, "negative_controls": controls}, stamps), {}
                )
        self.assertEqual(contracts.reusable_results({"tests": "corrupt"}, {"a": "one"}), {})

    def test_unchanged_contracts_are_reused_and_force_reruns_them(self):
        cases = [
            {
                "id": name,
                "runner": "rust",
                "package": "p",
                "target": ["--lib"],
                "test": name,
                "timeout": 5,
                "purpose": "fixture",
                "negative_control": {"file": "src/lib.rs"},
            }
            for name in ("a", "b")
        ]
        invoked, controlled = [], []

        def good(args, **kwargs):
            invoked.append(args[-6])
            return subprocess.CompletedProcess(
                args, 0, f"test {args[-6]} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;", ""
            )

        def controls(root, pending, fingerprints, evidence, record):
            for contract in pending:
                controlled.append(contract["id"])
                record(contracts.control_result(contract, fingerprints[contract["id"]], True, evidence / "x.log", 0))

        stamps = {"a": "one", "b": "two"}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with (
                patch.object(contracts, "contract_fingerprints", side_effect=lambda *args: dict(stamps)),
                patch.object(contracts.verify, "tool_identity", return_value={}),
                patch.object(contracts, "negative_controls", side_effect=controls),
                redirect_stdout(io.StringIO()),
            ):
                for change, fresh, expected in (
                    (None, False, ["a", "b"]),
                    (None, False, []),
                    ("b", False, ["b"]),
                    (None, True, ["a", "b"]),
                ):
                    invoked.clear()
                    controlled.clear()
                    if change:
                        stamps[change] += "-edited"
                    self.assertEqual(contracts.run(root, cases, good, policy={}, fresh=fresh), 0)
                    self.assertEqual((invoked, controlled), (expected, expected))
                    self.assertIn(
                        "normal_seconds",
                        json.loads((root / "target/verification/product-contracts.json").read_text())["execution"],
                    )
                    report = json.loads((root / "target/verification/product-contracts.json").read_text())
                    self.assertEqual(report["status"], "passed")
                    self.assertEqual(report["reused"], 2 - len(expected))
                    self.assertEqual(len(report["tests"]), 2)

    def test_a_batch_proves_only_a_test_with_its_own_single_pass(self):
        # ADR 0270: a task proves its required tests with one process per binary.
        rust = {"runner": "rust", "test": "a::b"}
        summary = "\ntest result: ok. 2 passed; 0 failed; 0 ignored;"
        self.assertTrue(contracts.proven_in_batch(rust, "test a::b ... ok\ntest a::c ... ok" + summary))
        for output in (
            "test a::b ... ignored" + summary,
            "test a::c ... ok" + summary,
            "test a::b ... ok",
            "test a::b ... FAILED" + summary,
            "test a::b ... ok\ntest a::b ... ok" + summary,
        ):
            with self.subTest(output=output):
                self.assertFalse(contracts.proven_in_batch(rust, output))
        swift = {"runner": "swift", "test": "contract"}
        run = "\nTest run with 2 tests in 2 suites passed"
        self.assertTrue(contracts.proven_in_batch(swift, "Test contract() passed after 0.1 seconds." + run))
        self.assertTrue(
            contracts.proven_in_batch(swift, "Test contract(case:) with 2 test cases passed after 0.1 seconds." + run)
        )
        for output in (
            "Test contract() skipped" + run,
            "Test contract() passed after 0.1 seconds with 1 known issue." + run,
            "Test contractTwo() passed after 0.1 seconds." + run,
            "Test contract() failed after 0.1 seconds." + run,
            "Test contract() passed after 0.1 seconds.",
            "Test contract() passed after 0.1 seconds.\nTest contract() passed after 0.1 seconds." + run,
        ):
            with self.subTest(output=output):
                self.assertFalse(contracts.proven_in_batch(swift, output))

    def test_a_task_runs_one_process_per_binary_and_retries_what_it_did_not_prove_alone(self):
        # ADR 0328: the binaries come from one `cargo test --no-run` of the default members,
        # the build check-base tests, and run from their package folder as `cargo test` runs them.
        cases = [
            {
                "id": name,
                "runner": "rust",
                "package": "echode-model",
                "target": ["--test", "t"],
                "test": name,
                "timeout": 5,
                "purpose": "fixture",
            }
            for name in ("a", "b", "c")
        ]
        cases.append(
            {
                "id": "d",
                "runner": "rust",
                "package": "echode-model",
                "target": ["--lib"],
                "test": "d",
                "timeout": 5,
                "purpose": "fixture",
            }
        )
        folder = ROOT / "crates/echode-model"
        built = [
            artifact(folder, ["test"], "t", "/bin/t-1"),
            artifact(folder, ["lib"], "echode_model", "/bin/lib-1"),
            # The library itself and another package's tests are no binary of these contracts.
            dict(artifact(folder, ["lib"], "echode_model", None), profile={"test": False}),
            artifact(ROOT / "crates/echode-time", ["test"], "t", "/bin/other-t"),
        ]
        invoked = []

        def fake(args, **kwargs):
            invoked.append((args, kwargs))
            if args[0] == "cargo":
                return subprocess.CompletedProcess(args, 0, "\n".join(map(json.dumps, built)) + "\n", "")
            names = args[1 : args.index("--exact")]
            lines = [f"test {name} ... {'FAILED' if name == 'b' and len(names) > 1 else 'ok'}" for name in names]
            return subprocess.CompletedProcess(
                args, 0, "\n".join(lines) + "\ntest result: ok. 1 passed; 0 failed; 0 ignored;", ""
            )

        with tempfile.TemporaryDirectory() as directory:
            results = []
            contracts.run_tests(ROOT, cases, fake, {}, Path(directory), results.append, batched=True)
        self.assertEqual(
            [args for args, _ in invoked if args[0] == "cargo"],
            [["cargo", "test", "--locked", "--offline", "--no-run", "--message-format=json-render-diagnostics"]],
        )
        runs = [(args, kwargs) for args, kwargs in invoked if args[0] != "cargo"]
        self.assertEqual(
            [(args[0], args[1 : args.index("--exact")], "--nocapture" in args) for args, _ in runs],
            [("/bin/t-1", ["a", "b", "c"], False), ("/bin/t-1", ["b"], True), ("/bin/lib-1", ["d"], True)],
        )
        for _args, kwargs in runs:
            self.assertEqual(kwargs["cwd"], folder)
            self.assertEqual(kwargs["env"]["CARGO_MANIFEST_DIR"], str(folder))
        self.assertEqual(
            {result["id"]: (result["status"], result.get("isolation"), result["build"]) for result in results},
            {
                "a": ("passed", "batch", "members"),
                "c": ("passed", "batch", "members"),
                "d": ("passed", None, "members"),
                "b": ("passed", None, "members"),
            },
        )

    def test_a_contract_with_features_builds_them_as_check_base_does(self):
        # ADR 0328: features need their own build; it is check-base's spike run, not one more copy.
        engine = {"id": "e", "package": "echode-engine", "target": ["--lib", "--features", "spike"]}
        selection, target = contracts.rust_selection(engine)
        self.assertEqual((selection, target), (("-p", "echode-engine", "--features", "spike"), ("--lib",)))
        self.assertEqual(
            contracts.rust_test_build(selection, [target, target])[-5:],
            ["-p", "echode-engine", "--features", "spike", "--lib"],
        )
        self.assertEqual(contracts.rust_selection({"id": "m", "package": "p", "target": ["--test", "t"]})[0], ())
        self.assertNotIn("--lib", contracts.rust_test_build((), [("--lib",)]), "the default members build it all")
        for target in (["--bin", "x"], ["--test"], ["--lib", "--test", "t"], ["--features", "spike"]):
            with self.subTest(target=target), self.assertRaisesRegex(ValueError, "--lib or --test NAME"):
                contracts.rust_selection({"id": "x", "package": "p", "target": target})
        policy = json.loads((ROOT / "scripts/verification.json").read_text())
        base = (ROOT / "Makefile").read_text().split("\ncheck-base:\n", 1)[1].split("\n\n", 1)[0]
        for contract in policy["product_contracts"]:
            if contract["runner"] == "rust" and "--features" in contract["target"]:
                with self.subTest(contract=contract["id"]):
                    selection, target = contracts.rust_selection(contract)
                    self.assertIn(f"cargo test -q {' '.join(selection)} {' '.join(target)}\n", base)

    def test_a_failed_or_incomplete_test_build_fails_each_contract_with_its_reason(self):
        cases = [
            {
                "id": name,
                "runner": "rust",
                "package": "echode-model",
                "target": target,
                "test": name,
                "timeout": 5,
                "purpose": "fixture",
            }
            for name, target in (("a", ["--lib"]), ("b", ["--lib"]), ("c", ["--test", "missing"]))
        ]
        folder = ROOT / "crates/echode-model"
        for code, expected in ((101, "error[E0425]"), (0, "built no --test missing test of echode-model")):
            invoked = []

            def fake(args, invoked=invoked, code=code, **kwargs):
                invoked.append(args)
                if args[0] != "cargo":
                    names = args[1 : args.index("--exact")]
                    lines = "".join(f"test {name} ... ok\n" for name in names)
                    return subprocess.CompletedProcess(args, 0, lines + "test result: ok. 2 passed;", "")
                lib = json.dumps(artifact(folder, ["lib"], "echode_model", "/bin/lib-1"))
                return subprocess.CompletedProcess(args, code, lib + "\n", "error[E0425]: cannot find value")

            with (
                self.subTest(code=code),
                tempfile.TemporaryDirectory() as directory,
                redirect_stdout(io.StringIO()),
                redirect_stderr(io.StringIO()),
            ):
                results = []
                contracts.run_tests(ROOT, cases, fake, {}, Path(directory), results.append, batched=True)
                statuses = {result["id"]: result["status"] for result in results}
                if code:
                    self.assertEqual(statuses, dict.fromkeys("abc", "failed"))
                    self.assertEqual(len(invoked), 1, "nothing runs from a failed build")
                else:
                    self.assertEqual(statuses, {"a": "passed", "b": "passed", "c": "failed"})
                self.assertIn(expected, (Path(directory) / "c.log").read_text())

    def test_swift_contracts_run_before_a_rust_test_build_relinks_the_helper(self):
        # ADR 0328: `cargo test` relinks target/debug/echode-plugin-host; Swift tests launch the shipped one.
        cases = [
            {
                "id": "rust-case",
                "runner": "rust",
                "package": "echode-model",
                "target": ["--lib"],
                "test": "r",
                "timeout": 5,
                "purpose": "fixture",
            },
            {
                "id": "swift-case",
                "runner": "swift",
                "package_path": "App",
                "suite": "Suite",
                "test": "s",
                "timeout": 5,
                "purpose": "fixture",
            },
        ]
        folder = ROOT / "crates/echode-model"
        for batched in (True, False):
            invoked = []

            def fake(args, invoked=invoked, **kwargs):
                invoked.append(args[0] if args[0] != "swift" else " ".join(args[:2]))
                if args[0] == "cargo" and "--no-run" in args:
                    lib = json.dumps(artifact(folder, ["lib"], "echode_model", "/bin/lib-1"))
                    return subprocess.CompletedProcess(args, 0, lib + "\n", "")
                if args[0] == "swift":
                    return subprocess.CompletedProcess(
                        args, 0, "Test s() passed\nTest run with 1 test in 1 suite passed", ""
                    )
                return subprocess.CompletedProcess(
                    args, 0, "test r ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;", ""
                )

            with self.subTest(batched=batched), tempfile.TemporaryDirectory() as directory:
                results = []
                contracts.run_tests(ROOT, cases, fake, {}, Path(directory), results.append, batched=batched)
                self.assertEqual(invoked[:2], ["swift build", "swift test"])
                self.assertEqual(invoked[2:], ["cargo", "/bin/lib-1"] if batched else ["cargo"])
                self.assertEqual([result["status"] for result in results], ["passed", "passed"])
                self.assertEqual(results[1]["build"], "members" if batched else "package")
                self.assertNotIn("build", results[0])

    def test_a_swift_batch_builds_once_and_filters_every_test_in_one_helper_run(self):
        cases = [
            {
                "id": name,
                "runner": "swift",
                "package_path": "swift/EchodeApp",
                "build_product": "EchodeAppTests",
                "suite": "S",
                "test": name,
                "timeout": 5,
                "purpose": "fixture",
            }
            for name in ("one", "two")
        ]
        invoked = []

        def fake(args, **kwargs):
            invoked.append(args)
            if args[:2] == ["swift", "build"]:
                return subprocess.CompletedProcess(args, 0, "", "")
            return subprocess.CompletedProcess(
                args,
                0,
                "Test one() passed after 0.1 seconds.\n"
                "Test two() passed after 0.1 seconds.\nTest run with 2 tests in 1 suite passed",
                "",
            )

        def batch(root, group, scratch):
            return ["helper", *[argument for case in group for argument in ("--filter", "S/" + case["test"])]], {}

        with (
            tempfile.TemporaryDirectory() as directory,
            patch.object(contracts, "swift_batch_invocation", side_effect=batch),
            patch.object(contracts, "test_invocation", return_value=(["unused"], None)),
        ):
            results = []
            contracts.run_tests(ROOT, cases, fake, {}, Path(directory), results.append, batched=True)
        self.assertEqual(
            [args[0] if args[0] == "helper" else " ".join(args[:2]) for args in invoked], ["swift build", "helper"]
        )
        self.assertEqual(invoked[1].count("--filter"), 2)
        self.assertEqual(
            [(result["id"], result["status"], result["isolation"]) for result in results],
            [("one", "passed", "batch"), ("two", "passed", "batch")],
        )

    def test_a_task_runs_no_counterexample_and_each_tier_reuses_only_its_own_evidence(self):
        cases = [
            {
                "id": name,
                "runner": "rust",
                "package": "p",
                "target": ["--lib"],
                "test": name,
                "timeout": 5,
                "purpose": "fixture",
                "negative_control": {"file": "src/lib.rs"},
            }
            for name in ("a", "b")
        ]
        tested, controlled = [], []

        def run_tests(_root, pending, _invoke, stamps, _evidence, record, batched=False):
            # As run_tests records them: a task proves "a" in a batch and "b" alone (ADR 0271).
            for case in pending:
                tested.append((case["id"], batched))
                record(
                    {
                        "id": case["id"],
                        "status": "passed",
                        "fingerprint": stamps[case["id"]],
                        **({"isolation": "batch"} if batched and case["id"] == "a" else {}),
                        **contracts.evidence_build(case, batched),
                    }
                )

        def controls(root, pending, fingerprints, evidence, record):
            for contract in pending:
                controlled.append(contract["id"])
                record(contracts.control_result(contract, fingerprints[contract["id"]], True, evidence / "x.log", 0))

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with (
                patch.object(contracts, "contract_fingerprints", side_effect=lambda *args: {"a": "1", "b": "2"}),
                patch.object(contracts.verify, "tool_identity", return_value={}),
                patch.object(contracts, "run_tests", side_effect=run_tests),
                patch.object(contracts, "negative_controls", side_effect=controls),
                redirect_stdout(io.StringIO()),
            ):
                self.assertEqual(contracts.run(root, cases, policy={}, tests_only=True), 0)
                self.assertEqual(contracts.run(root, cases, policy={}, tests_only=True), 0)
                # ADR 0327: the counterexamples build mutated copies, so only the full tier runs them.
                self.assertEqual(controlled, [])
                report = json.loads((root / "target/verification/product-contracts.json").read_text())
                self.assertEqual(report["execution"]["control_scope"], "none")
                # ADR 0328: the full tier proves each Rust test alone in its own package's build,
                # a task in the default members' build. Neither stands for the other.
                self.assertEqual(contracts.run(root, cases, policy={}), 0)
                self.assertEqual(contracts.run(root, cases, policy={}, tests_only=True), 0)
                report = json.loads((root / "target/verification/product-contracts.json").read_text())
                self.assertEqual(
                    [item["status"] for item in report["negative_controls"]],
                    ["passed", "passed"],
                    "a task keeps the full tier's current counterexample passes on record",
                )
                self.assertEqual(contracts.run(root, cases, policy={}, tests_only=True), 0)
        self.assertEqual(tested, [("a", True), ("b", True), ("a", False), ("b", False), ("a", True), ("b", True)])
        self.assertEqual(controlled, ["a", "b"])

    def test_shards_split_every_counterexample_once(self):
        # ADR 0269: the full tier runs its counterexamples in several GitHub jobs.
        cases = [{"id": name} for name in "abcdefg"]
        for count in range(1, 5):
            with self.subTest(count=count):
                shards = [contracts.shard_contracts(cases, f"{index}/{count}") for index in range(count)]
                self.assertEqual(set().union(*shards), set("abcdefg"))
                self.assertEqual(sum(len(shard) for shard in shards), len(cases))
        for value in ("", "1", "2/2", "-1/2", "0/0", "01/2", "0/2/3", "a/2"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                contracts.shard_contracts(cases, value)

    def test_a_shard_runs_every_test_but_only_its_own_counterexamples(self):
        cases = [
            {
                "id": name,
                "runner": "rust",
                "package": "p",
                "target": ["--lib"],
                "test": name,
                "timeout": 5,
                "purpose": "fixture",
                "negative_control": {"file": "src/lib.rs"},
            }
            for name in "abc"
        ]
        invoked, controlled = [], []

        def good(args, **kwargs):
            invoked.append(args[-6])
            return subprocess.CompletedProcess(
                args, 0, f"test {args[-6]} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;", ""
            )

        def controls(root, pending, fingerprints, evidence, record):
            for contract in pending:
                controlled.append(contract["id"])
                record(contracts.control_result(contract, fingerprints[contract["id"]], True, evidence / "x.log", 0))

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with (
                patch.object(
                    contracts, "contract_fingerprints", side_effect=lambda *args: {"a": "1", "b": "2", "c": "3"}
                ),
                patch.object(contracts.verify, "tool_identity", return_value={}),
                patch.object(contracts, "negative_controls", side_effect=controls),
                redirect_stdout(io.StringIO()),
            ):
                self.assertEqual(contracts.run(root, cases, good, policy={}, shard="1/2"), 0)
                with self.assertRaises(ValueError):
                    contracts.run(root, cases, good, policy={}, shard="1/2", tests_only=True)
        self.assertEqual((invoked, controlled), (["a", "b", "c"], ["b"]))

    def test_an_edit_during_the_run_leaves_no_reusable_result(self):
        case = {
            "id": "a",
            "runner": "rust",
            "package": "p",
            "target": ["--lib"],
            "test": "a",
            "timeout": 5,
            "purpose": "fixture",
            "negative_control": {"file": "src/lib.rs"},
        }
        stamps = iter([{"a": "before"}, {"a": "before"}, {"a": "edited"}, {"a": "edited"}])

        def good(args, **kwargs):
            return subprocess.CompletedProcess(
                args, 0, "test a ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;", ""
            )

        def controls(root, pending, fingerprints, evidence, record):
            record(contracts.control_result(pending[0], fingerprints["a"], True, evidence / "x.log", 0))

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with (
                patch.object(contracts, "contract_fingerprints", side_effect=lambda *args: next(stamps)),
                patch.object(contracts.verify, "tool_identity", return_value={}),
                patch.object(contracts, "negative_controls", side_effect=controls),
                redirect_stderr(io.StringIO()),
            ):
                self.assertEqual(contracts.run(root, [case], good, policy={}), 1)
            report = json.loads((root / "target/verification/product-contracts.json").read_text())
            self.assertEqual(report["status"], "failed")
            self.assertEqual(
                [(item["status"], item["fingerprint"]) for item in report["tests"] + report["negative_controls"]],
                [("stale", None), ("stale", None)],
            )
            self.assertEqual(contracts.reusable_results(report, {"a": "before"}), {})

    def test_contract_identities_follow_only_their_own_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            git_repository(root)
            (root / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/*"]\n')
            for name, dependencies in (("a", ""), ("b", 'a = { path = "../a" }\n')):
                (root / f"crates/{name}/src").mkdir(parents=True)
                (root / f"crates/{name}/Cargo.toml").write_text(
                    f'[package]\nname = "{name}"\nversion = "0.1.0"\n\n[dependencies]\n{dependencies}'
                )
                (root / f"crates/{name}/src/lib.rs").write_text("// fixture\n")
            (root / "swift/App").mkdir(parents=True)
            (root / "swift/App/View.swift").write_text("// fixture\n")
            (root / "docs").mkdir()
            (root / "docs/notes.md").write_text("fixture\n")
            (root / "Makefile").write_text("check-product-contracts:\n\tpython3 run.py\n")

            def control(path):
                return {"file": path, "old": "fixture", "new": "changed"}

            cases = [
                {
                    "id": "rust-a",
                    "runner": "rust",
                    "package": "a",
                    "target": ["--lib"],
                    "test": "t",
                    "timeout": 5,
                    "negative_control": control("crates/a/src/lib.rs"),
                },
                {
                    "id": "rust-b",
                    "runner": "rust",
                    "package": "b",
                    "target": ["--lib"],
                    "test": "t",
                    "timeout": 5,
                    "negative_control": control("crates/b/src/lib.rs"),
                },
                {
                    "id": "swift",
                    "runner": "swift",
                    "package_path": "swift/App",
                    "suite": "S",
                    "test": "t",
                    "timeout": 5,
                    "negative_control": control("swift/App/View.swift"),
                },
            ]
            policy = {
                "version": 1,
                "global_inputs": ["Cargo.toml"],
                "infrastructure": [],
                "evidence_inputs": [],
                "areas": {},
                "product_contracts": cases,
                "product_contract_inputs": {
                    "common": {"inputs": [], "packages": []},
                    "rust": {"inputs": [], "packages": []},
                    "swift": {"inputs": ["swift/**"], "packages": ["b"]},
                },
                "checks": [
                    {
                        "id": "check-product-contracts",
                        "mode": "automatic",
                        "areas": [],
                        "timeout": 5,
                        "inputs": ["swift/**"],
                        "packages": ["b"],
                        "policy_inputs": ["product_contracts", "product_contract_inputs"],
                    }
                ],
            }

            def stamps(tools=None):
                return contracts.contract_fingerprints(root, policy, cases, tools or {"rustc": "v1"})

            def changed(before, after):
                return {name for name in before if before[name] != after[name]}

            initial = stamps()
            (root / "docs/notes.md").write_text("edited\n")
            self.assertEqual(changed(initial, stamps()), set())
            (root / "crates/b/src/lib.rs").write_text("// edited\n")
            after_b = stamps()
            self.assertEqual(changed(initial, after_b), {"rust-b", "swift"})
            (root / "swift/App/View.swift").write_text("// edited\n")
            after_swift = stamps()
            self.assertEqual(changed(after_b, after_swift), {"swift"})
            (root / "crates/a/src/lib.rs").write_text("// edited\n")
            self.assertEqual(changed(after_swift, stamps()), {"rust-a", "rust-b", "swift"})
            current = stamps()
            policy["checks"][0]["timeout"] += 10
            self.assertEqual(changed(current, stamps()), set(), "a wrapper budget does not change contract evidence")
            current = stamps()
            cases[0]["timeout"] = 6
            self.assertEqual(changed(current, stamps()), {"rust-a"})
            self.assertEqual(changed(current, stamps({"rustc": "v2"})), {"rust-a", "rust-b", "swift"})
            (root / "Makefile").write_text(
                "# A comment does not change the recipe.\n" + (root / "Makefile").read_text()
            )
            current = stamps()
            (root / "Makefile").write_text("check-product-contracts:\n\tpython3 other.py\n")
            self.assertEqual(changed(current, stamps()), {"rust-a", "rust-b", "swift"})
            cases[0]["negative_control"] = control("docs/notes.md")
            with self.assertRaises(ValueError):
                stamps()

    def test_child_processes_stay_in_the_outer_verification_group(self):
        process = Mock(returncode=0)
        process.communicate.return_value = ("", "")
        with (
            patch.dict(contracts.os.environ, {"ECHODE_VERIFY_GROUP": "1"}),
            patch.object(contracts.os, "getsid", return_value=42),
            patch.object(contracts.os, "getpgrp", return_value=42),
            patch.object(contracts.subprocess, "Popen", return_value=process) as popen,
        ):
            contracts.run_process(["test"], cwd=ROOT, capture_output=True, text=True, timeout=1)
            self.assertFalse(popen.call_args.kwargs["start_new_session"])

    def test_a_direct_run_owns_and_stops_its_child_group(self):
        process = Mock(pid=1234)
        process.communicate.side_effect = [subprocess.TimeoutExpired(["test"], 1), ("", "")]
        with (
            patch.dict(contracts.os.environ, {"ECHODE_VERIFY_GROUP": ""}),
            patch.object(contracts.subprocess, "Popen", return_value=process) as popen,
            patch.object(contracts.os, "killpg") as killpg,
        ):
            with self.assertRaises(subprocess.TimeoutExpired):
                contracts.run_process(["test"], cwd=ROOT, capture_output=True, text=True, timeout=1)
            self.assertTrue(popen.call_args.kwargs["start_new_session"])
            killpg.assert_called_once_with(1234, contracts.signal.SIGKILL)

    def test_rust_requires_one_unignored_named_test(self):
        case = {"runner": "rust", "test": "path::contract"}
        good = "test path::contract ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured;"
        self.assertTrue(contracts.passed(case, 0, good))
        for output in (
            "test result: ok. 0 passed; 0 failed; 1 ignored;",
            "running 0 tests",
            good.replace("path::contract", "path::other"),
            good.replace("... ok", "... ignored"),
        ):
            self.assertFalse(contracts.passed(case, 0, output))
        self.assertFalse(contracts.passed(case, 1, good))

    def test_swift_does_not_accept_empty_xctest_or_disabled_tests(self):
        case = {"runner": "swift", "test": "contractHeader"}
        good = "✔ Test contractHeader() passed after 0.01 seconds.\n✔ Test run with 1 test passed after 0.02 seconds."
        self.assertTrue(contracts.passed(case, 0, good))
        for output in (
            "Executed 0 tests, with 0 failures",
            "✔ Test run with 0 tests passed",
            "◇ Test contractHeader() skipped",
            good.replace("contractHeader", "differentTest"),
            good + "\nknown issue",
        ):
            self.assertFalse(contracts.passed(case, 0, output))

    def test_swift_parameterized_contracts_require_executed_cases(self):
        case = {"runner": "swift", "test": "contractDiagnostics"}
        for count, label in ((1, "case"), (2, "cases")):
            good = (
                f"✔ Test contractDiagnostics(status:) with {count} test {label} passed after 0.01 seconds.\n"
                "✔ Test run with 1 test in 1 suite passed after 0.02 seconds."
            )
            self.assertTrue(contracts.passed(case, 0, good), "parameterized required test was not recognized")
            failed = "Expectation failed\n" + good.replace("passed", "failed")
            self.assertTrue(contracts.rejected_behavior(case, 1, failed))
            self.assertFalse(contracts.passed(case, 1, good))
            self.assertFalse(contracts.rejected_behavior(case, 0, failed))
            for output in (
                good.replace("contractDiagnostics", "otherTest"),
                good.replace(f"with {count} test {label} ", ""),
                good.replace(f"with {count} test {label}", "with 0 test cases"),
                good.replace("passed", "skipped"),
                good + "\nknown issue",
                good.replace("run with 1 test", "run with 0 tests"),
                good.replace("run with 1 test", "run with 2 tests"),
            ):
                self.assertFalse(contracts.passed(case, 0, output))
            for output in (
                failed.replace("contractDiagnostics", "otherTest"),
                failed.replace(f"with {count} test {label}", "with 0 test cases"),
                failed.replace("Expectation failed", "compiler error"),
            ):
                self.assertFalse(contracts.rejected_behavior(case, 1, output))

    def test_missing_test_stops_the_gate_and_preserves_evidence(self):
        case = {
            "id": "required",
            "runner": "rust",
            "package": "p",
            "target": ["--lib"],
            "test": "contract",
            "timeout": 1,
            "purpose": "test the gate",
        }
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
        self.assertTrue(
            contracts.rejected_behavior(
                rust, 101, "test contract ... FAILED\ntest result: FAILED. 0 passed; 1 failed; 0 ignored;"
            )
        )
        self.assertTrue(
            contracts.rejected_behavior(
                swift, 1, "Expectation failed\nTest contract() failed\nTest run with 1 test in 1 suite failed"
            )
        )
        for case in (rust, swift):
            self.assertFalse(contracts.rejected_behavior(case, 1, "compiler error"))
            self.assertFalse(contracts.rejected_behavior(case, 1, "missing compiler"))
            self.assertFalse(contracts.rejected_behavior(case, 0, "passed"))
            # An M1 runner that cannot execute an M4 instruction crashes the test; that is no detection.
            self.assertFalse(contracts.rejected_behavior(case, -4, "Illegal instruction: 4"))

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
