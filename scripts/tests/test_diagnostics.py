"""Diagnostics must distinguish stale evidence and avoid exporting raw settings."""

import fcntl
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[2]
with patch.object(sys, "path", [str(ROOT / "scripts"), *sys.path]):
    spec = importlib.util.spec_from_file_location("diagnose", ROOT / "scripts/diagnose.py")
    diagnose = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(diagnose)


class DiagnosticTests(unittest.TestCase):
    def setUp(self):
        # A parent's base ref belongs to its repository, not these Git fixtures.
        environment = patch.dict(os.environ)
        environment.start()
        self.addCleanup(environment.stop)
        os.environ.pop("VERIFY_BASE", None)
        self.temp = tempfile.TemporaryDirectory(prefix="echode-diagnosis-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "scripts").mkdir()
        policy = {"version": 1, "global_inputs": [], "infrastructure": [], "areas": {}, "checks": [
            {"id": "check", "mode": "automatic", "always": True, "areas": [], "timeout": 10}]}
        (self.root / "scripts/verification.json").write_text(json.dumps(policy))
        (self.root / ".gitignore").write_text("/target\n")
        (self.root / "source.txt").write_text("original")
        for args in (["init", "--quiet"], ["add", "."],
                     ["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--quiet", "-m", "fixture"]):
            subprocess.run(["git", *args], cwd=self.root, check=True, capture_output=True)
        (self.root / "source.txt").write_text("edit")
        self.tools = {key: "0:fixture" for key in ("rustc -vV", "cargo --version", "swift --version", "xcodebuild -version")}
        self.tools["RUSTFLAGS"] = "SECRET_TEST_SETTING"
        self.addCleanup(patch.stopall)
        patch.object(diagnose.verify, "tool_identity", return_value=self.tools).start()
        patch.object(diagnose.shutil, "which", return_value="/fixture/tool").start()

    def save_report(self, status="passed"):
        directory = self.root / "target/verification"
        directory.mkdir(parents=True, exist_ok=True)
        changed = diagnose.verify.changed_paths(self.root)
        stamp = diagnose.verify.fingerprint(self.root, changed, None, self.tools)
        (directory / "report.json").write_text(json.dumps({"version": 1, "fingerprint": stamp, "base": None,
            "tools": self.tools, "results": {"check": {"status": status}}}))

    def test_current_partial_and_stale_results_are_distinct(self):
        self.assertEqual(diagnose.collect(self.root)["verification"]["freshness"], "unavailable")
        self.save_report("pending")
        data = diagnose.collect(self.root)
        self.assertEqual(data["verification"]["freshness"], "current")
        self.assertEqual(data["verification"]["checks"][0]["status"], "pending")
        (self.root / "source.txt").write_text("later edit")
        data = diagnose.collect(self.root)
        self.assertEqual(data["verification"]["freshness"], "stale")
        self.assertEqual(data["verification"]["checks"][0]["status"], "unknown")

    def test_corrupt_report_is_diagnosable_and_environment_is_not_exported(self):
        self.save_report()
        report = self.root / "target/verification/report.json"
        report.write_text("{invalid")
        data = diagnose.collect(self.root)
        self.assertEqual(data["verification"]["freshness"], "unavailable")
        self.assertTrue(data["verification"]["error"])
        self.assertNotIn("SECRET_TEST_SETTING", json.dumps(data))
        report.write_text(json.dumps({"version": 1, "results": []}))
        self.assertEqual(diagnose.collect(self.root)["verification"]["error"], "invalid verification record")

    def test_installed_but_broken_toolchain_is_reported(self):
        self.tools["rustc +nightly-fixture -vV"] = "1:toolchain is missing"
        data = diagnose.collect(self.root)
        self.assertEqual(data["environment"]["missing_tools"], [])
        self.assertIn("rustc +nightly-fixture -vV", data["environment"]["tool_errors"])

    def test_explicit_base_is_resolved_in_the_repository_being_diagnosed(self):
        reference = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=self.root, text=True).strip()
        with patch.dict(os.environ, {"VERIFY_BASE": reference}):
            data = diagnose.collect(self.root)
        self.assertEqual(data["base"], reference)
        self.assertEqual(data["changed_paths"], ["source.txt"])
        with patch.dict(os.environ, {"VERIFY_BASE": "missing-fixture-ref"}):
            with self.assertRaises(subprocess.CalledProcessError):
                diagnose.collect(self.root)

    def test_persistent_lock_file_is_idle_until_it_is_actually_locked(self):
        self.save_report()
        path = self.root / "target/verification/.lock"
        path.touch()
        self.assertFalse(diagnose.collect(self.root)["verification"]["running"])
        with path.open("a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            self.assertTrue(diagnose.collect(self.root)["verification"]["running"])
        self.assertFalse(diagnose.collect(self.root)["verification"]["running"])

    def test_build_provenance_expires_for_source_changes_and_replaced_binaries(self):
        stamp = diagnose.source_stamp(self.root)
        for name in diagnose.BUILD_FILES:
            path = self.root / "target/Echode.app" / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"fixture binary")
        diagnose.record_build(self.root, stamp)
        self.assertEqual(diagnose.build_status(self.root, stamp)["status"], "current")
        (self.root / "source.txt").write_text("changed during build")
        with self.assertRaisesRegex(ValueError, "source changed"):
            diagnose.record_build(self.root, stamp)
        self.assertEqual(diagnose.build_status(self.root, diagnose.source_stamp(self.root))["status"], "stale")
        (self.root / "target/Echode.app" / diagnose.BUILD_FILES[0]).write_bytes(b"different binary")
        result = diagnose.build_status(self.root, stamp)
        self.assertEqual(result["status"], "stale")
        self.assertFalse(result["binaries_match"])

    def test_bundle_is_bounded_keeps_originals_and_does_not_follow_external_links(self):
        self.save_report()
        log = self.root / "target/verification/check.log"
        log.write_bytes(b"0123456789" * 10)
        outside = self.root / "private-project.json"
        outside.write_text("DO_NOT_COLLECT")
        (self.root / "target/spike-external.json").symlink_to(outside)
        data = diagnose.collect(self.root)
        with patch.object(diagnose, "LOG_LIMIT", 16):
            output = diagnose.bundle(self.root, data)
        self.assertEqual((output / "verification/check.log").read_bytes(), log.read_bytes()[-16:])
        self.assertEqual(log.stat().st_size, 100)
        self.assertFalse((output / "spike-external.json").exists())
        record = json.loads((output / "diagnosis.json").read_text())
        self.assertIn({"path": "verification/check.log", "truncated": True}, record["artifacts"])
        self.assertNotIn("SECRET_TEST_SETTING", (output / "diagnosis.json").read_text())
        self.assertFalse((output / "verification/report.json").exists())


class DeveloperDocumentationTests(unittest.TestCase):
    def test_maintained_guides_link_to_existing_local_files_and_make_targets(self):
        guides = ["AGENTS.md", "README.md", "docs/README.md", "docs/status.md",
                  "docs/development.md", "docs/debugging.md", "docs/verification.md"]
        targets = set(re.findall(r"^([a-z][a-z0-9-]*):", (ROOT / "Makefile").read_text(), re.MULTILINE))
        for name in guides:
            path = ROOT / name
            content = path.read_text()
            for destination in re.findall(r"\]\(([^)]+)\)", content):
                if "://" in destination or destination.startswith("#"):
                    continue
                relative = unquote(destination.split("#", 1)[0])
                with self.subTest(guide=name, link=relative):
                    self.assertTrue((path.parent / relative).exists(), "broken local link")
            for target in re.findall(r"\bmake ([a-z](?:[a-z0-9-]*[a-z0-9])?)(?=[\s`.,;]|$)", content):
                with self.subTest(guide=name, target=target):
                    self.assertIn(target, targets, "unknown make target")


if __name__ == "__main__":
    unittest.main()
