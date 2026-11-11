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
    from documentation import anchor_errors, heading_anchors, local_link_errors


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
            {"id": "check", "mode": "automatic", "always": True, "areas": [], "inputs": ["**"], "timeout": 10}]}
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
        self.runtime = self.root / "target/toolchains/tsan"
        patch.object(diagnose.tsan_runtime, "CACHE", self.runtime).start()
        self.runtime_context = patch.object(diagnose.tsan_runtime, "context", return_value={"clang": "fixture"}).start()

    def save_report(self, status="passed"):
        directory = self.root / "target/verification"
        directory.mkdir(parents=True, exist_ok=True)
        changed = diagnose.verify.changed_paths(self.root)
        policy = diagnose.verify.load_policy(self.root)
        stamp = diagnose.verify.check_fingerprints(self.root, policy, self.tools)["check"]
        (directory / "report.json").write_text(json.dumps({"version": 2, "base": None,
            "tools": self.tools, "results": {"check": {"status": status, "fingerprint": stamp}}}))

    def test_current_partial_and_stale_results_are_distinct(self):
        self.assertEqual(diagnose.collect(self.root)["verification"]["freshness"], "unavailable")
        self.save_report("pending")
        data = diagnose.collect(self.root)
        self.assertEqual(data["verification"]["freshness"], "current")
        self.assertEqual(data["verification"]["checks"][0]["status"], "pending")
        (self.root / "source.txt").write_text("later edit")
        data = diagnose.collect(self.root)
        self.assertEqual(data["verification"]["freshness"], "stale")
        self.assertEqual(data["verification"]["checks"][0]["status"], "stale")

    def test_unrelated_check_stays_current_when_only_documentation_changes(self):
        path = self.root / "scripts/verification.json"
        policy = json.loads(path.read_text())
        policy["checks"].append({"id": "audio", "mode": "automatic", "always": True,
                                 "areas": [], "inputs": ["source.txt"], "timeout": 10})
        path.write_text(json.dumps(policy))
        directory = self.root / "target/verification"
        directory.mkdir(parents=True)
        stamps = diagnose.verify.check_fingerprints(self.root, policy, self.tools)
        report = {"version": 2, "results": {name: {"status": "passed", "fingerprint": stamp}
                                            for name, stamp in stamps.items()}}
        (directory / "report.json").write_text(json.dumps(report))
        (self.root / "README.md").write_text("updated instructions")
        data = diagnose.collect(self.root)["verification"]
        self.assertEqual(data["freshness"], "partial")
        self.assertEqual([check["status"] for check in data["checks"]], ["stale", "passed"])

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

    def test_missing_or_outdated_tsan_runtime_asks_for_bridge_setup(self):
        # The cache can disappear while every tool is installed; then the TSan
        # checks show stale with no hint unless the doctor names the setup.
        self.assertEqual(diagnose.collect(self.root)["environment"]["setup_needed"], ["setup-bridge"])
        libraries = {}
        for variant in ("baseline", "fixed"):
            library = self.runtime / variant / diagnose.tsan_runtime.LIBRARY
            library.parent.mkdir(parents=True)
            library.write_bytes(variant.encode())
            libraries[variant] = diagnose.tsan_runtime.sha256(library)
        (self.runtime / "manifest.json").write_text(json.dumps({"context": {"clang": "fixture"}, "libraries": libraries}))
        data = diagnose.collect(self.root)
        self.assertEqual(data["environment"]["setup_needed"], [])
        self.assertIn("準備のやり直し: なし", diagnose.render(data))
        self.runtime_context.return_value = {"clang": "updated Xcode"}
        data = diagnose.collect(self.root)
        self.assertEqual(data["environment"]["setup_needed"], ["setup-bridge"])
        self.assertIn("make setup-bridge", diagnose.render(data))
        self.runtime_context.side_effect = FileNotFoundError("cmake")
        self.assertEqual(diagnose.collect(self.root)["environment"]["setup_needed"], ["setup-bridge"])

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
        plugin = self.root / "target/Echode.app" / diagnose.BUILD_FILES[2]
        plugin.write_bytes(b"different plugin")
        self.assertFalse(diagnose.build_status(self.root, stamp)["binaries_match"])
        plugin.write_bytes(b"fixture binary")
        (self.root / "target/Echode.app" / diagnose.BUILD_FILES[0]).write_bytes(b"different binary")
        result = diagnose.build_status(self.root, stamp)
        self.assertEqual(result["status"], "stale")
        self.assertFalse(result["binaries_match"])
        (self.root / "target/Echode.app" / diagnose.BUILD_FILES[0]).write_bytes(b"fixture binary")
        cli = self.root / "target/Echode.app/Contents/MacOS/echode-cli"
        cli.write_bytes(b"different export CLI")
        self.assertFalse(diagnose.build_status(self.root, stamp)["binaries_match"])
        cli.unlink()
        self.assertFalse(diagnose.build_status(self.root, stamp)["binaries_match"])

    def test_record_build_without_an_expected_source_is_a_usage_error(self):
        result = subprocess.run([sys.executable, "-B", str(ROOT / "scripts/diagnose.py"), "--record-build"],
                                capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 2)
        self.assertIn("--record-build requires --expected-source", result.stderr)
        self.assertNotIn("source changed", result.stderr)

    def test_markdown_used_as_a_build_input_still_invalidates_app_provenance(self):
        # A future embedded document is a dependency even though its suffix is .md.
        (self.root / "docs").mkdir()
        embedded = self.root / "docs/embedded.md"
        embedded.write_text("First embedded help")
        (self.root / "embedded.rs").write_text('const HELP: &str = include_str!("docs/embedded.md");\n')
        stamp = diagnose.source_stamp(self.root)
        for name in diagnose.BUILD_FILES:
            path = self.root / "target/Echode.app" / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"fixture binary")
        diagnose.record_build(self.root, stamp)
        embedded.write_text("Changed embedded help")
        self.assertEqual(diagnose.build_status(self.root, diagnose.source_stamp(self.root))["status"], "stale")

    def test_bundle_collects_declared_preflight_logs_but_not_arbitrary_command_output(self):
        path = self.root / "scripts/verification.json"
        policy = json.loads(path.read_text())
        policy["preflights"] = {name: {"command": ["true"], "timeout": 5, "remedy": "prepare fixture"}
                                for name in ("ready", "external")}
        path.write_text(json.dumps(policy))
        self.save_report()
        directory = self.root / "target/verification"
        (directory / "preflight-ready.log").write_text("missing runtime")
        (directory / "failure-latency.json").write_text('{"synthetic":true}')
        (directory / "timing-history.jsonl").write_text('{"execution":{"seconds":12}}\n')
        outside = self.root / "private-document"
        outside.write_text("DO_NOT_COLLECT")
        (directory / "preflight-external.log").symlink_to(outside)
        (directory / "preflight-unknown.log").write_text("DO_NOT_COLLECT")
        focused = directory / "focused"
        focused.mkdir()
        (focused / "1.log").write_text("DO_NOT_COLLECT")
        with patch.object(diagnose, "APP_LOG_DIR", self.root / "missing-app-logs"):
            output = diagnose.bundle(self.root, diagnose.collect(self.root))
        self.assertEqual((output / "verification/preflight-ready.log").read_text(), "missing runtime")
        self.assertTrue((output / "verification/failure-latency.json").is_file())
        self.assertEqual((output / "verification/timing-history.jsonl").read_text(), '{"execution":{"seconds":12}}\n')
        self.assertFalse((output / "verification/preflight-external.log").exists())
        self.assertFalse((output / "verification/preflight-unknown.log").exists())
        self.assertFalse((output / "verification/focused").exists())

    def test_bundle_is_bounded_keeps_originals_and_does_not_follow_external_links(self):
        self.save_report()
        log = self.root / "target/verification/check.log"
        log.write_bytes(b"0123456789" * 10)
        outside = self.root / "private-project.json"
        outside.write_text("DO_NOT_COLLECT")
        (self.root / "target/spike-external.json").symlink_to(outside)
        capture = self.root / "target/stop-diagnostics/captures"
        capture.mkdir(parents=True)
        (capture / "stop-example.json").write_text('{"pcm": "stop-example.f32le"}')
        (capture / "stop-example.f32le").write_bytes(b"USER_PLAYBACK_DO_NOT_COLLECT")
        runtime = self.root / "target/toolchains/tsan"
        runtime.mkdir(parents=True)
        (runtime / "manifest.json").write_text("pinned-runtime")
        (runtime / "runtime.dylib").write_bytes(b"DO_NOT_BUNDLE_RUNTIME")
        control = self.root / "target/bridge/thread/errno"
        control.mkdir(parents=True)
        (control / "real-race.log").write_text("errno-control")
        app_logs = self.root / "home-logs"
        app_logs.mkdir()
        (app_logs / "audition.ndjson").write_text('{"request":1}\n' * 4)
        (app_logs / "audition.1.ndjson").symlink_to(outside)
        (app_logs / "panic.ndjson").write_text('{"panic":1}\n')
        (app_logs / "echode.ndjson").write_text('{"log":1}\n')
        data = diagnose.collect(self.root)
        with patch.object(diagnose, "LOG_LIMIT", 16), patch.object(diagnose, "APP_LOG_DIR", app_logs):
            output = diagnose.bundle(self.root, data)
        self.assertEqual((output / "verification/check.log").read_bytes(), log.read_bytes()[-16:])
        self.assertEqual(log.stat().st_size, 100)
        self.assertFalse((output / "spike-external.json").exists())
        record = json.loads((output / "diagnosis.json").read_text())
        self.assertIn({"path": "verification/check.log", "truncated": True}, record["artifacts"])
        self.assertNotIn("SECRET_TEST_SETTING", (output / "diagnosis.json").read_text())
        self.assertFalse((output / "verification/report.json").exists())
        self.assertFalse((output / "stop-diagnostics").exists())
        self.assertEqual((output / "toolchains/tsan/manifest.json").read_text(), "pinned-runtime")
        self.assertEqual((output / "bridge/thread/errno/real-race.log").read_text(), "errno-control")
        self.assertFalse((output / "toolchains/tsan/runtime.dylib").exists())
        # The app's audition record is copied, bounded; a link out of its folder is not followed.
        self.assertEqual((output / "app-logs/audition.ndjson").read_text(), ('{"request":1}\n' * 4)[-16:])
        self.assertEqual(record["app_logs"], [{"path": "app-logs/echode.ndjson", "truncated": False},
                                              {"path": "app-logs/audition.ndjson", "truncated": True},
                                              {"path": "app-logs/panic.ndjson", "truncated": False}])
        self.assertEqual((output / "app-logs/panic.ndjson").read_text(), '{"panic":1}\n')
        self.assertEqual((output / "app-logs/echode.ndjson").read_text(), '{"log":1}\n')
        self.assertFalse((output / "app-logs/audition.1.ndjson").exists())
        self.assertIn("app-logs/", (output / "README.md").read_text())


class DeveloperDocumentationTests(unittest.TestCase):
    def test_new_target_links_are_rejected_even_when_the_file_exists(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "docs/review.md"
            (root / "target").mkdir()
            (root / "target/evidence.log").touch()
            links = (
                "[proof](../target/evidence.log)",
                "[proof](../%74arget/evidence.log#result)",
                "[proof](<../target/evidence.log>)",
                "[proof][record]\n[record]: ../target/evidence.log",
                "<../target/evidence.log>",
            )
            for link in links:
                with self.subTest(link=link):
                    errors = local_link_errors(root, path, link, {})
                    self.assertTrue(errors, "new target evidence link must be rejected")

    def test_file_url_links_are_rejected(self):
        # ENV-07: a file URL names one machine's path and was never checked.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "docs/review.md"
            for link in ("[here](file:///Users/someone/Echode/docs/status.md)", "[here](FILE:///tmp/x)"):
                with self.subTest(link=link):
                    errors = local_link_errors(root, path, link, {})
                    self.assertTrue(any("file URL" in error for error in errors), errors)

    def test_legacy_links_require_both_a_notice_and_document_scoped_allowance(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "docs/review.md"
            link = "[old](../target/missing.log)"
            legacy = {"docs/review.md": ["target/missing.log"]}
            notice = "当時のローカル記録\n"
            self.assertEqual(local_link_errors(root, path, notice + link, legacy), [])
            self.assertTrue(local_link_errors(root, path, link, legacy))
            self.assertTrue(local_link_errors(root, root / "docs/new.md", notice + link, legacy))
            self.assertTrue(local_link_errors(root, path, notice + "[new](../target/new.log)", legacy))

    def test_durable_links_are_checked_and_external_links_are_ignored(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "docs/review.md"
            root.joinpath("proof.txt").touch()
            self.assertEqual(local_link_errors(root, path, "[proof](../proof.txt#result)", {}), [])
            self.assertTrue(local_link_errors(root, path, "[proof](../missing.txt)", {}))
            self.assertEqual(local_link_errors(root, path, "[web](https://example.org) [here](#title)", {}), [])

    def test_maintained_guides_link_to_existing_local_files_and_make_targets(self):
        guides = [
            "AGENTS.md",
            "README.md",
            "docs/README.md",
            "docs/status.md",
            "docs/development.md",
            "docs/agent-workflow.md",
            "docs/debugging.md",
            *sorted(f"docs/debugging/{path.name}" for path in (ROOT / "docs/debugging").glob("*.md")),
            "docs/reviews/README.md",
            "docs/user-checks.md",
            "docs/verification.md",
            "docs/PLAN.md",
            "docs/ARCHITECTURE.md",
            "docs/phase2.md",
            "docs/interaction-design.md",
            "docs/architecture/model-and-history.md",
            "docs/architecture/audio-engine.md",
            "docs/architecture/plugins-and-devices.md",
            "docs/architecture/ui-and-metal.md",
            "docs/architecture/api-qa-and-distribution.md",
        ]
        targets = set(re.findall(r"^([a-z][a-z0-9-]*):", (ROOT / "Makefile").read_text(), re.MULTILINE))
        for name in guides:
            path = ROOT / name
            content = path.read_text()
            for lineno, line in enumerate(content.splitlines(), start=1):
                with self.subTest(guide=name, line=lineno):
                    self.assertLessEqual(
                        len(line.encode("utf-8")),
                        600,
                        f"{name}:{lineno} exceeds 600 UTF-8 bytes ({len(line.encode('utf-8'))} B)",
                    )
            for destination in re.findall(r"\]\(([^)]+)\)", content):
                if "://" in destination or destination.startswith("#"):
                    continue
                relative = unquote(destination.split("#", 1)[0])
                with self.subTest(guide=name, link=relative):
                    self.assertTrue((path.parent / relative).exists(), "broken local link")
            for target in re.findall(r"\bmake ([a-z](?:[a-z0-9-]*[a-z0-9])?)(?=[\s`.,;]|$)", content):
                with self.subTest(guide=name, target=target):
                    self.assertIn(target, targets, "unknown make target")

    def test_records_link_to_existing_files_except_local_evidence(self):
        legacy = json.loads((ROOT / "docs/evidence/legacy-local-links.json").read_text())
        paths = sorted((ROOT / "docs").rglob("*.md")) + [ROOT / "README.md", ROOT / "AGENTS.md"]
        for path in paths:
            with self.subTest(document=str(path.relative_to(ROOT))):
                self.assertEqual(local_link_errors(ROOT, path, path.read_text(), legacy), [])
                # A moved or renamed heading must not leave a link that silently lands at the top.
                self.assertEqual(anchor_errors(ROOT, path, path.read_text()), [])

    def test_heading_anchors_follow_the_github_rules(self):
        text = ("# 要求の外枠の検証（2026-10-29）\n## UI・描画・入力\n## `make verify` の所要時間\n"
                "## [リンク](x.md) の見出し\n## 重複\n## 重複\n```\n# fenced\n```\n")
        self.assertEqual(heading_anchors(text), {"要求の外枠の検証2026-10-29", "ui描画入力", "make-verify-の所要時間",
                                                 "リンク-の見出し", "重複", "重複-1"})

    def test_missing_heading_anchors_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "docs").mkdir()
            (root / "docs/guide.md").write_text("# Guide\n\n## 保存・復旧\n")
            path = root / "docs/review.md"
            content = "# Review\n[ok](guide.md#保存復旧) [self](#review) [web](https://example.org/#x)\n"
            self.assertEqual(anchor_errors(root, path, content), [])
            for link in ("[gone](guide.md#保存-復旧)", "[gone](#missing)", "[gone](guide.md#)"):
                with self.subTest(link=link):
                    self.assertTrue(anchor_errors(root, path, content + link))

    def test_debugging_index_lists_every_symptom_heading(self):
        index = (ROOT / "docs/debugging.md").read_text()
        listed = set(re.findall(r"^  - (.+)$", index, re.MULTILINE))
        files = sorted((ROOT / "docs/debugging").glob("*.md"))
        self.assertTrue(files)
        symptoms = set()
        for path in files:
            with self.subTest(area=path.name):
                self.assertIn(f"](debugging/{path.name})", index, "area file missing from the index")
                symptoms |= set(re.findall(r"^### (.+)$", path.read_text(), re.MULTILINE))
        self.assertEqual(listed, symptoms, "the symptom index and the area files differ")

    def test_makefile_comments_stay_short(self):
        # XCA-10: per-feature comment logs grew above recipes; contracts live in verification.json.
        run = 0
        for lineno, line in enumerate((ROOT / "Makefile").read_text().splitlines(), start=1):
            run = run + 1 if line.startswith("#") else 0
            with self.subTest(line=lineno):
                self.assertLessEqual(run, 3, "Makefile comment block exceeds 3 lines")


if __name__ == "__main__":
    unittest.main()
