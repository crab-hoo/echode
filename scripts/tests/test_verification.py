"""Required checks fail closed; optional measurements do not block completion."""

import argparse
from contextlib import redirect_stderr, redirect_stdout
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import select
import shlex
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / filename)
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


verify = module("verify", "verify.py")
hook = module("check_on_stop", "check_on_stop.py")


def arguments(**overrides):
    options = dict(base=None, all=False, only=[], run=[], defer=[], reason=None, plan=False, force=False)
    options.update(overrides)
    return argparse.Namespace(**options)


class PolicyTests(unittest.TestCase):
    def setUp(self):
        self.policy = verify.load_policy(ROOT)

    def selected(self, *paths):
        return set(verify.select_checks(self.policy, list(paths)))

    def test_model_changes_include_transitive_bridge_audio_and_measurements(self):
        checks = self.selected("crates/echode-model/src/time.rs")
        self.assertTrue({"check", "rtsan", "check-bridge-features", "check-bridge-asan",
                         "check-bridge-tsan", "check-bridge-miri", "spike-engine",
                         "spike-history", "spike-draw", "spike-bridge", "spike-render"} <= checks)

    def test_bridge_changes_do_not_launch_unrelated_hardware(self):
        checks = self.selected("spikes/bridge/swift/CBridge.swift")
        self.assertTrue({"check-bridge-asan", "check-bridge-tsan", "check-bridge-miri", "spike-bridge"} <= checks)
        self.assertNotIn("spike-input", checks)
        self.assertNotIn("rtsan", checks)

    def test_product_bridge_and_app_select_ownership_checks(self):
        for path in ("crates/echode-ffi/src/queue.rs", "swift/EchodeBridge/Sources/EchodeBridge/Document.swift",
                     "swift/EchodeApp/Sources/EchodeAppCore/ProjectController.swift"):
            with self.subTest(path=path):
                self.assertTrue({"check", "check-bridge-asan", "check-bridge-tsan", "check-bridge-miri"} <= self.selected(path))
                self.assertTrue({"rtsan", "spike-engine", "spike-audio"} <= self.selected(path))

    def test_product_audio_selects_realtime_and_an_explicit_silent_hal_probe(self):
        for path in ("crates/echode-audio-io/src/stream.rs", "crates/echode-engine/src/playback.rs",
                     "swift/EchodeBridge/Sources/EchodeBridge/Audio.swift"):
            with self.subTest(path=path):
                self.assertTrue({"check", "rtsan", "spike-engine", "spike-audio"} <= self.selected(path))
                self.assertNotIn("spike-input", self.selected(path))
                if path.startswith("crates/echode-audio-io"):
                    self.assertIn("check-bridge-asan", self.selected(path))

    def test_product_drawing_has_a_separate_real_display_probe(self):
        self.assertIn("spike-app", self.selected("swift/EchodeApp/Sources/EchodeAppCore/ArrangeView.swift"))
        for file in ("PianoRollView.swift", "PianoGeometry.swift"):
            with self.subTest(file=file):
                checks = self.selected("swift/EchodeApp/Sources/EchodeAppCore/" + file)
                self.assertTrue({"spike-app", "spike-draw", "check-bridge-asan", "check-bridge-tsan", "check-bridge-miri"} <= checks)
        self.assertNotIn("spike-app", self.selected("crates/echode-engine/src/voice.rs"))

    def test_input_has_both_headless_tests_and_an_explicit_hardware_check(self):
        self.assertEqual(self.selected("spikes/input/Sources/EchodeInputCore/Gesture.swift"),
                         {"check", "check-input", "spike-input"})

    def test_isolation_and_dependency_changes_select_their_extra_checks(self):
        checks = self.selected("crates/echode-sandbox/src/lib.rs")
        self.assertTrue({"rtsan", "spike-engine", "spike-isolation"} <= checks)
        self.assertIn("advisories", self.selected("crates/echode-api/Cargo.toml"))

    def test_common_settings_do_not_require_unrelated_measurements(self):
        automatic = {check["id"] for check in self.policy["checks"] if check["mode"] == "automatic"}
        for path in self.policy["global_inputs"]:
            with self.subTest(path=path):
                expected = automatic | ({"advisories"} if path in ("Cargo.toml", "Cargo.lock", "deny.toml") else set())
                self.assertEqual(self.selected(path), expected)
        self.assertEqual(set(verify.select_checks(self.policy, [], all_checks=True)),
                         {check["id"] for check in self.policy["checks"]})

    def test_infrastructure_runs_all_portable_checks_without_opening_windows(self):
        expected = {check["id"] for check in self.policy["checks"] if check["mode"] == "automatic"}
        for path in ("scripts/verify.py", ".cursor/hooks.json", ".github/workflows/check.yml", "Makefile"):
            self.assertEqual(self.selected(path), expected)
        self.assertEqual(self.selected("docs/PLAN.md"), {"check"})

    def test_policy_references_real_targets_and_never_rewrites_sound_references(self):
        targets = set(re.findall(r"^([a-z][a-z0-9-]*):", (ROOT / "Makefile").read_text(), re.MULTILINE))
        ids = {check["id"] for check in self.policy["checks"]}
        self.assertTrue(ids <= targets)
        self.assertNotIn("bless", ids)
        self.assertNotIn("known-bugs", ids)

    def test_github_matrix_is_generated_from_the_automatic_checks(self):
        with tempfile.TemporaryDirectory(prefix="echode-matrix-") as temporary:
            output = Path(temporary) / "github-output"
            result = subprocess.run([sys.executable, str(ROOT / "scripts/verify.py"), "--github-matrix"],
                                    env={**os.environ, "GITHUB_OUTPUT": str(output)},
                                    capture_output=True, text=True, check=True)
            expected = [check["id"] for check in self.policy["checks"] if check["mode"] == "automatic"]
            self.assertEqual(json.loads(result.stdout), expected)
            self.assertEqual(json.loads(output.read_text().removeprefix("checks=")), expected)


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="echode-verification-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "scripts").mkdir()
        (self.root / "src").mkdir()
        (self.root / ".gitignore").write_text("/target\n")
        (self.root / "src/example.rs").write_text("original\n")
        (self.root / "Makefile").write_text("check:\n\t@echo check\n\nmeasure:\n\t@echo measure\n")
        policy = {"version": 1, "global_inputs": [], "infrastructure": [],
                  "areas": {"source": ["src/**"]}, "checks": [
                      {"id": "check", "mode": "automatic", "always": True, "areas": [], "timeout": 10, "purpose": "fixture"},
                      {"id": "measure", "mode": "manual", "areas": ["source"], "timeout": 10, "purpose": "fixture"}]}
        (self.root / verify.POLICY).write_text(json.dumps(policy))
        self.git("init", "--quiet")
        self.git("add", ".")
        # Commits belong only to this disposable fixture repository.
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--quiet", "-m", "fixture")
        (self.root / "src/example.rs").write_text("first edit\n")

    def git(self, *args):
        return verify.git(self.root, *args)

    def report(self):
        return json.loads((self.root / "target/verification/report.json").read_text())

    def run_verify(self, options, runner=None, tools=None):
        runner = runner or (lambda *args: {"status": "passed"})
        with patch.object(verify, "tool_identity", return_value=tools or {"rust": "fixture-v1"}), \
             patch.object(verify, "run_check", side_effect=runner) as invocation, \
             redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
            code = verify.verify(self.root, options)
        return code, invocation.call_count

    def test_changed_paths_include_staged_untracked_deleted_and_renamed_files_with_spaces(self):
        self.git("mv", "src/example.rs", "src/renamed file.rs")
        (self.root / "src/new file.rs").write_text("untracked")
        (self.root / "Makefile").unlink()
        self.assertEqual(verify.changed_paths(self.root),
                         ["Makefile", "src/example.rs", "src/new file.rs", "src/renamed file.rs"])

    def test_committed_changes_require_an_explicit_base(self):
        base = self.git("rev-parse", "HEAD").strip().decode()
        self.git("add", ".")
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--quiet", "-m", "edit")
        self.assertEqual(verify.changed_paths(self.root), [])
        self.assertEqual(verify.changed_paths(self.root, base), ["src/example.rs"])
        with self.assertRaises(subprocess.CalledProcessError):
            verify.changed_paths(self.root, "missing-ref")

    def test_deferral_is_not_a_pass_and_expires_after_another_edit(self):
        code, count = self.run_verify(arguments(defer=["measure"], reason="device unavailable"))
        self.assertEqual((code, count), (0, 1))
        self.assertEqual(self.report()["results"]["measure"]["status"], "deferred")
        code, count = self.run_verify(arguments())
        self.assertEqual((code, count), (0, 0))
        (self.root / "src/example.rs").write_text("second edit\n")
        code, count = self.run_verify(arguments())
        self.assertEqual((code, count), (0, 1))
        self.assertEqual(self.report()["results"]["measure"], {"status": "not_run"})

    def test_unrequested_measurements_do_not_trigger_a_stop_followup(self):
        def invoke():
            code, count = self.run_verify(arguments())
            self.assertEqual(count, 1)
            return code
        self.assertEqual(hook.respond({"status": "completed"}, invoke), {})
        self.assertEqual(self.report()["results"]["check"]["status"], "passed")
        self.assertEqual(self.report()["results"]["measure"]["status"], "not_run")
        self.assertEqual(self.run_verify(arguments()), (0, 0))
        self.assertEqual(self.run_verify(arguments(all=True)), (0, 0))

    def test_explicit_measurements_must_pass_and_failures_are_not_hidden_by_the_hook(self):
        self.assertEqual(self.run_verify(arguments(only=["measure"])), (1, 0))
        self.assertEqual(self.run_verify(arguments(run=["measure"])), (0, 2))
        self.assertEqual(self.report()["results"]["measure"]["status"], "passed")
        def fail_measurement(root, check, log):
            return {"status": "failed" if check["id"] == "measure" else "passed"}
        self.assertEqual(self.run_verify(arguments(run=["measure"], force=True), fail_measurement), (1, 2))
        response = hook.respond({"status": "completed"}, lambda: self.run_verify(arguments())[0])
        self.assertIn("followup_message", response)
        self.assertEqual(self.run_verify(arguments(run=["measure"])), (0, 1))

    def test_required_failures_still_trigger_a_stop_followup(self):
        response = hook.respond({"status": "completed"}, lambda: self.run_verify(
            arguments(), lambda *args: {"status": "failed", "exit_code": 7})[0])
        self.assertIn("followup_message", response)
        self.assertEqual(self.report()["results"]["check"]["status"], "failed")

    def test_automatic_checks_cannot_be_deferred_or_forged_by_an_empty_reason(self):
        with self.assertRaises(ValueError):
            self.run_verify(arguments(defer=["check"], reason="skip"))
        with self.assertRaises(ValueError):
            self.run_verify(arguments(defer=["measure"], reason=" "))
        with self.assertRaises(ValueError):
            self.run_verify(arguments(only=["unknown"]))

    def test_cached_success_expires_when_an_unchanged_dependency_or_tool_changes(self):
        options = arguments(only=["check"])
        self.assertEqual(self.run_verify(options), (0, 1))
        self.assertEqual(self.run_verify(options), (0, 0))
        (self.root / "Makefile").write_text("check:\n\t@echo changed dependency\n")
        self.assertEqual(self.run_verify(options), (0, 1))
        self.assertEqual(self.run_verify(options, tools={"rust": "fixture-v2"}), (0, 1))

    def test_concurrent_source_edits_cannot_be_recorded_as_success(self):
        def edit_while_running(*args):
            (self.root / "src/example.rs").write_text("edited while testing\n")
            return {"status": "passed"}
        self.assertEqual(self.run_verify(arguments(only=["check"]), edit_while_running), (1, 1))
        self.assertEqual(self.report()["results"]["check"]["status"], "stale")

    def test_failed_checks_are_recorded_and_run_again(self):
        failure = lambda *args: {"status": "failed", "exit_code": 7}
        options = arguments(only=["check"])
        self.assertEqual(self.run_verify(options, failure), (1, 1))
        self.assertEqual(self.run_verify(options), (0, 1))

    def test_actual_command_failure_keeps_its_diagnostic(self):
        (self.root / "Makefile").write_text("check:\n\t@echo deliberate failure\n\t@exit 7\n")
        log = self.root / "failure.log"
        result = verify.run_check(self.root, {"id": "check", "timeout": 10}, log)
        self.assertEqual(result["status"], "failed")
        self.assertIn("deliberate failure", log.read_text())

    def test_timeout_stops_the_child_process_group(self):
        # This checks watchdog behavior, not a product performance threshold.
        pipe = self.root / "child.fifo"
        os.mkfifo(pipe)
        reader = os.open(pipe, os.O_RDONLY | os.O_NONBLOCK)
        self.addCleanup(os.close, reader)
        (self.root / "child.py").write_text(
            "import os, time\n"
            "from pathlib import Path\n"
            "Path('child.pid').write_text(str(os.getpid()))\n"
            "writer = os.open('child.fifo', os.O_WRONLY)\n"
            "os.write(writer, b'ready')\n"
            "time.sleep(60)\n")
        def cleanup_child():
            pid_file = self.root / "child.pid"
            if pid_file.exists():
                try:
                    os.kill(int(pid_file.read_text()), signal.SIGKILL)
                except ProcessLookupError:
                    pass
        self.addCleanup(cleanup_child)
        (self.root / "Makefile").write_text(f"check:\n\t@{shlex.quote(sys.executable)} parent.py\n")
        log = self.root / "timeout.log"
        for outer_timeout, inner_timeout in ((3, 60), (10, 1)):
            with self.subTest(outer=outer_timeout, inner=inner_timeout):
                (self.root / "parent.py").write_text(
                    "import subprocess, sys\n"
                    f"subprocess.run([sys.executable, {str(ROOT / 'scripts/run-bridge-validation.py')!r}, "
                    f"'--timeout', '{inner_timeout}', '--log', 'child.log', sys.executable, 'child.py'])\n")
                result = verify.run_check(self.root, {"id": "check", "timeout": outer_timeout}, log)
                self.assertEqual(result["status"], "failed")
                if outer_timeout < inner_timeout:
                    self.assertEqual(result["error"], "timeout")
                    self.assertIn("VERIFICATION TIMEOUT", log.read_text())
                else:
                    self.assertIn("VALIDATION TIMEOUT", (self.root / "child.log").read_text())
                self.assertEqual(os.read(reader, 10), b"ready")
                readable, _, _ = select.select([reader], [], [], 3)
                self.assertTrue(readable, "the child still holds its writer open")
                self.assertEqual(os.read(reader, 10), b"")


class HookTests(unittest.TestCase):
    def test_only_the_completed_status_starts_verification(self):
        self.assertEqual(hook.respond({"status": "cancelled", "text": '"status":"completed"'},
                                      lambda: self.fail("should not run")), {})
        self.assertEqual(hook.respond({"status": "completed"}, lambda: 0), {})
        self.assertIn("followup_message", hook.respond({"status": "completed"}, lambda: 1))

    def test_real_hook_protocol_is_json_and_invalid_input_does_not_silently_pass(self):
        for event, expected in ((json.dumps({"status": "cancelled"}), {}), ("{invalid", None)):
            result = subprocess.run([str(ROOT / ".cursor/hooks/check-on-stop.sh")], cwd=ROOT,
                                    input=event, capture_output=True, text=True, check=True)
            response = json.loads(result.stdout)
            if expected is None:
                self.assertIn("followup_message", response)
            else:
                self.assertEqual(response, expected)


if __name__ == "__main__":
    unittest.main()
