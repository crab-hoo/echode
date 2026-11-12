"""Compact context must retain obligations and source excerpts must be exact."""

import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
with patch.object(sys, "path", [str(ROOT / "scripts"), *sys.path]):
    spec = importlib.util.spec_from_file_location("agent_context", ROOT / "scripts/agent_context.py")
    context = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(context)


class ContextTests(unittest.TestCase):
    def setUp(self):
        environment = patch.dict(os.environ)
        environment.start()
        self.addCleanup(environment.stop)
        os.environ.pop("VERIFY_BASE", None)
        temporary = tempfile.TemporaryDirectory(prefix="echode-agent-context-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / "scripts").mkdir()
        self.policy = {
            "version": 1,
            "global_inputs": [],
            "infrastructure": ["scripts/**"],
            "areas": {area: [f"{area}/**"] for area in context.AREA_GUIDES},
            "checks": [
                {
                    "id": "check",
                    "mode": "automatic",
                    "always": True,
                    "areas": [],
                    "inputs": ["**"],
                    "timeout": 10,
                    "purpose": "fixture contract",
                },
                {
                    "id": "measure",
                    "mode": "manual",
                    "areas": ["audio"],
                    "inputs": ["audio/**"],
                    "timeout": 10,
                    "purpose": "fixture measurement",
                },
            ],
        }
        (self.root / "scripts/verification.json").write_text(json.dumps(self.policy))
        (self.root / ".gitignore").write_text("/target\n")
        for name in set(context.COMMON_GUIDES) | {name for names in context.AREA_GUIDES.values() for name in names}:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("# Guide\n\n## Boundary\nEvidence\n")
        for args in (
            ["init", "--quiet"],
            ["add", "."],
            [
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ],
        ):
            subprocess.run(["git", *args], cwd=self.root, check=True, capture_output=True)
        (self.root / "audio").mkdir()
        (self.root / "audio/source.rs").write_text("// fixture input\n")
        tools = patch.object(context.diagnose.verify, "tool_identity", return_value={"RUSTFLAGS": "PRIVATE_VALUE"})
        self.tool_identity = tools.start()
        self.addCleanup(tools.stop)
        setup = patch.object(context.diagnose, "setup_needed", return_value=[])
        setup.start()
        self.addCleanup(setup.stop)

    def test_collection_uses_one_diagnosis_and_the_existing_selection(self):
        data = context.collect(self.root)
        self.tool_identity.assert_called_once()
        selected = context.verify.select_checks(self.policy, data["diagnosis"]["changed_paths"])
        self.assertEqual(set(data["selection"]), set(selected))
        for name, reasons in selected.items():
            self.assertEqual(data["selection"][name]["reasons"], reasons)
        self.assertEqual(data["areas"], ["audio"])
        self.assertNotIn("PRIVATE_VALUE", json.dumps(data))
        self.assertEqual({check["id"] for check in data["diagnosis"]["verification"]["checks"]}, set(selected))

    def test_requested_guides_add_to_detected_areas_without_changing_checks(self):
        data = context.collect(self.root, ["history"])
        self.assertEqual(data["areas"], ["audio", "history"])
        self.assertEqual(set(data["selection"]), {"check", "measure"})
        paths = {guide["path"] for guide in data["guides"]}
        self.assertIn("docs/architecture/model-and-history.md", paths)
        self.assertTrue(set(context.COMMON_GUIDES).issubset(paths))
        with self.assertRaisesRegex(ValueError, "unknown area"):
            context.collect(self.root, ["typo"])

    def test_old_report_base_does_not_change_a_new_plan_and_explicit_base_is_respected(self):
        directory = self.root / "target/verification"
        directory.mkdir(parents=True)
        (directory / "report.json").write_text(json.dumps({"version": 2, "base": "old-missing-ref", "results": {}}))
        data = context.collect(self.root)
        self.assertIsNone(data["diagnosis"]["base"])
        reference = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=self.root, text=True).strip()
        with patch.dict(os.environ, {"VERIFY_BASE": reference}):
            self.assertEqual(context.collect(self.root)["diagnosis"]["base"], reference)
        with patch.dict(os.environ, {"VERIFY_BASE": "explicit-missing-ref"}):
            with self.assertRaises(subprocess.CalledProcessError):
                context.collect(self.root)

    def test_changed_collection_missing_guides_and_unmapped_areas_fail_closed(self):
        with patch.object(context.diagnose, "source_stamp", side_effect=["before", "before", "after"]):
            with self.assertRaisesRegex(ValueError, "changed during"):
                context.collect(self.root)
        (self.root / "docs/README.md").unlink()
        with self.assertRaises(FileNotFoundError):
            context.collect(self.root)
        self.policy["areas"]["new-domain"] = ["new/**"]
        (self.root / "scripts/verification.json").write_text(json.dumps(self.policy))
        with self.assertRaisesRegex(ValueError, "documentation areas differ"):
            context.collect(self.root)

    def test_snapshot_is_complete_and_holds_exact_guide_hashes(self):
        data = context.collect(self.root)
        context.save(self.root, data)
        saved = json.loads((self.root / context.SNAPSHOT).read_text())
        self.assertEqual(saved, data)
        for guide in saved["guides"]:
            self.assertEqual(guide["sha256"], hashlib.sha256((self.root / guide["path"]).read_bytes()).hexdigest())
        self.assertEqual(list((self.root / "target").iterdir()), [self.root / "target/agent-context.json"])

    def test_summary_retains_all_nonpassing_states_and_groups_only_equal_deferrals(self):
        data = context.collect(self.root)
        checks = []
        for status in ("failed", "stale", "pending", "running", "not_run", "unknown"):
            checks.append(
                {
                    "id": "auto-" + status,
                    "mode": "automatic",
                    "status": status,
                    "fresh": True,
                    "log": f"target/verification/{status}.log",
                }
            )
        checks += [
            {"id": "good", "mode": "automatic", "status": "passed", "fresh": True},
            {"id": "old-pass", "mode": "automatic", "status": "passed", "fresh": False, "log": "old.log"},
        ]
        for name, reason in (
            ("manual-one", "User owns hardware"),
            ("manual-two", "User owns hardware"),
            ("manual-three", "Different reason"),
            ("manual-invalid", ""),
        ):
            checks.append({"id": name, "mode": "manual", "status": "deferred", "fresh": True, "reason": reason})
        data["diagnosis"]["verification"].update(checks=checks, error="corrupt report", running=True)
        data["diagnosis"]["environment"].update(
            missing_tools=["cargo"], tool_errors=["rustc -vV"], setup_needed=["setup-bridge"]
        )
        output = context.render(data)
        for check in checks:
            self.assertIn(check["id"], output)
            if check.get("log"):
                self.assertIn(check["log"], output)
        self.assertIn("1/8 current passes", output)
        self.assertEqual(output.count("User owns hardware"), 1)
        for signal in (
            "corrupt report",
            "cargo",
            "rustc -vV",
            "make setup-bridge",
            "BUSY",
            "INVALID deferral",
            "make verify",
        ):
            self.assertIn(signal, output)

    def test_reading_budget_reports_the_start_reads_and_oversized_sections(self):
        big = self.root / "docs/development.md"
        big.write_text("# Guide\n\n## Large\n" + "x" * context.SECTION_BUDGET_BYTES + "\n## Small\nok\n")
        status = self.root / "docs/status.md"
        # Many small sections: only the start total exceeds its budget.
        status.write_text("# Status\n\n" + ("## Part\n" + "y" * 1000 + "\n") * (context.START_BUDGET_BYTES // 1000))
        data = context.collect(self.root)
        reading = data["reading"]
        self.assertEqual([item["path"] for item in reading["start"]], list(context.START_GUIDES))
        self.assertGreater(reading["start_bytes"], context.START_BUDGET_BYTES)
        self.assertEqual(
            [(item["path"], item["title"]) for item in reading["oversized"]], [("docs/development.md", "Large")]
        )
        output = context.render(data)
        self.assertIn("OVER BUDGET: the full reads", output)
        self.assertIn("OVER BUDGET: docs/development.md:3 Large", output)
        status.write_text("# Status\n")
        big.write_text("# Guide\n\n## Large\n" + "x" * (context.SECTION_BUDGET_BYTES - 100) + "\n")
        self.assertNotIn("OVER BUDGET", context.render(context.collect(self.root)))

    def test_live_guides_stay_within_the_reading_budget(self):
        # XCA-10: raising a budget needs an ADR with the reason and measurements.
        reading = context.reading_budget(ROOT, context.all_guides())
        self.assertLessEqual(reading["start_bytes"], context.START_BUDGET_BYTES, reading["start"])
        self.assertEqual(reading["oversized"], [])

    def test_live_documentation_index_covers_every_verification_area(self):
        policy = context.verify.load_policy(ROOT)
        self.assertEqual(set(context.AREA_GUIDES), set(policy["areas"]))
        automatic = {check["id"] for check in policy["checks"] if check["mode"] == "automatic"}
        self.assertTrue(automatic.issubset(context.verify.select_checks(policy, ["scripts/verify.py"], full=True)))
        # The context tool is tested by check-verification; it does not change any check's evidence.
        always = {check["id"] for check in policy["checks"] if check.get("always")}
        self.assertEqual(set(context.verify.select_checks(policy, ["scripts/agent_context.py"])), always)
        for name in set(context.COMMON_GUIDES) | {name for paths in context.AREA_GUIDES.values() for name in paths}:
            self.assertTrue((ROOT / name).is_file(), name)


class SourceViewTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="echode-source-view-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.text = (
            "# Guide\nBefore\n## Target\n日本語\n### Nested\nDetail\n```sh\n## Not a heading\n```\n## Next\nAfter\n"
        )
        (self.root / "guide.md").write_text(self.text)

    def test_nested_section_is_exact_and_fenced_examples_do_not_end_it(self):
        view = context.source_view(self.root, "guide.md", section="Target")
        self.assertEqual((view["start"], view["end"]), (3, 9))
        self.assertEqual(view["content"], "".join(self.text.splitlines(keepends=True)[2:9]))
        outline = context.source_view(self.root, "guide.md", outline=True)
        self.assertEqual([item["title"] for item in outline["headings"]], ["Guide", "Target", "Nested", "Next"])

    def test_old_line_numbers_require_matching_source_when_a_hash_is_supplied(self):
        outline = context.source_view(self.root, "guide.md", outline=True)
        (self.root / "guide.md").write_text("Inserted\n" + self.text)
        with self.assertRaisesRegex(ValueError, "source changed"):
            context.source_view(self.root, "guide.md", line_range="3:4", expected=outline["source"]["sha256"])

    def test_limits_refuse_content_instead_of_returning_partial_success(self):
        with self.assertRaisesRegex(ValueError, "no content was returned"):
            context.source_view(self.root, "guide.md", max_bytes=8)
        full = context.source_view(self.root, "guide.md", max_bytes=10000)
        self.assertEqual(full["content"], self.text)
        # The budget counts UTF-8 bytes, not characters, and has no product limit.
        with self.assertRaises(ValueError):
            context.source_view(self.root, "guide.md", line_range="4:4", max_bytes=4)
        self.assertEqual(context.source_view(self.root, "guide.md", line_range="4:4")["content"], "日本語\n")

    def test_ambiguous_sections_invalid_ranges_and_external_links_are_rejected(self):
        (self.root / "guide.md").write_text(self.text + "## Target\nDuplicate\n")
        for kwargs in (
            {"section": "Target"},
            {"section": "Missing"},
            {"line_range": "0:2"},
            {"line_range": "5:2"},
            {"line_range": "1:999"},
            {"max_bytes": 0},
        ):
            with self.subTest(kwargs=kwargs), self.assertRaises(ValueError):
                context.source_view(self.root, "guide.md", **kwargs)
        with tempfile.TemporaryDirectory(prefix="echode-private-") as outside:
            private = Path(outside) / "private.md"
            private.write_text("PRIVATE_DOCUMENT")
            (self.root / "external.md").symlink_to(private)
            for name in (str(private), "external.md"):
                with self.subTest(name=name), self.assertRaises(ValueError):
                    context.source_view(self.root, name)

    def test_code_ranges_and_json_cli_preserve_exact_text(self):
        (self.root / "source.rs").write_bytes(b"fn example() {\r\n    value();\r\n}")
        view = context.source_view(self.root, "source.rs", line_range="2:3")
        self.assertEqual(view["content"], "    value();\r\n}")
        self.assertEqual(context.render_view(view).partition("\n")[2], view["content"])
        result = subprocess.run(
            [
                sys.executable,
                "-B",
                str(ROOT / "scripts/agent_context.py"),
                "--read",
                "docs/development.md",
                "--section",
                "検査をまとめて進める",
                "--json",
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(json.loads(result.stdout)["content"].startswith("## 検査をまとめて進める\n"))

    def test_cli_refusal_has_nonzero_exit_and_does_not_leak_partial_content(self):
        for mode in ("--read", "--outline"):
            with self.subTest(mode=mode):
                result = subprocess.run(
                    [
                        sys.executable,
                        "-B",
                        str(ROOT / "scripts/agent_context.py"),
                        mode,
                        "AGENTS.md",
                        "--max-bytes",
                        "1",
                    ],
                    cwd=ROOT,
                    capture_output=True,
                    text=True,
                    timeout=10,
                )
                self.assertEqual(result.returncode, 1)
                self.assertEqual(result.stdout, "")
                self.assertIn("no content was returned", result.stderr)

    def test_metadata_and_short_lines_cannot_escape_the_display_budget(self):
        (self.root / "short.md").write_text("\n" * 1000)
        view = context.source_view(self.root, "short.md", max_bytes=1500)
        self.assertLessEqual(len(context.render_view(view, max_bytes=1500).encode()), 1500)
        with self.assertRaisesRegex(ValueError, "formatted view"):
            context.render_view(view, as_json=True, max_bytes=1500)


if __name__ == "__main__":
    unittest.main()
