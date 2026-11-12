"""ADR 0323: Swift, Python and shell sources are formatted and linted by pinned or recorded tools."""

import hashlib
import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

import lint


class Languages(unittest.TestCase):
    def test_the_suffix_decides_before_the_first_line(self):
        self.assertEqual(lint.language(Path("a/B.swift"), ""), "swift")
        self.assertEqual(lint.language(Path("a/b.py"), "#!/bin/bash"), "python")
        self.assertEqual(lint.language(Path("a/b.sh"), ""), "shell")
        self.assertIsNone(lint.language(Path("a/b.env"), ""))

    def test_a_script_without_a_suffix_is_known_by_its_interpreter(self):
        for line, expected in (
            ("#!/bin/bash", "shell"),
            ("#!/usr/bin/env bash", "shell"),
            ("#!/usr/bin/env -S bash -eu", "shell"),
            ("#!/bin/sh", "shell"),
            ("#!/usr/bin/env python3", "python"),
            ("#!/usr/bin/env zsh", None),
            ("# not a shebang", None),
            ("#!", None),
        ):
            with self.subTest(line=line):
                self.assertEqual(lint.language(Path("tool"), line), expected)

    def test_the_repository_sources_are_found_without_ignored_or_deleted_files(self):
        found = lint.sources(ROOT)
        self.assertIn("scripts/lint.py", found["python"])
        self.assertIn("scripts/fuzz.sh", found["shell"])
        self.assertIn("swift/EchodeApp/Sources/EchodeApp/EchodeApp.swift", found["swift"])
        for names in found.values():
            for name in names:
                self.assertFalse(name.startswith("target/"), name)
                self.assertTrue((ROOT / name).is_file(), name)

    def test_the_generated_swift_actions_are_left_to_their_own_check(self):
        # check-swift-actions compares the file byte for byte with the generator's output.
        header = (ROOT / "swift/EchodeBridge/Sources/EchodeBridge/EditAction.swift").read_text().split("import ", 1)[0]
        self.assertIn("// swift-format-ignore-file\n", header)


class PinnedTools(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.folder = Path(temporary.name)
        self.requirements = self.folder / "lint-tools.txt"
        self.requirements.write_text("ruff==1\n")
        self.tools = self.folder / "tools"

    def install(self, stamp, binaries=lint.PINNED):
        (self.tools / "bin").mkdir(parents=True)
        (self.tools / lint.STAMP.name).write_text(stamp + "\n")
        for name in binaries:
            (self.tools / "bin" / name).write_text("")

    def current(self):
        return hashlib.sha256(self.requirements.read_bytes()).hexdigest()

    def test_a_missing_or_stale_install_is_refused(self):
        self.assertEqual(len(lint.missing_tools(self.requirements, self.tools)), 1)
        self.install("stale")
        self.assertIn("changed", lint.missing_tools(self.requirements, self.tools)[0])

    def test_a_partial_install_names_each_missing_tool(self):
        self.install(self.current(), binaries=("ruff",))
        self.assertEqual(lint.missing_tools(self.requirements, self.tools), ["shellcheck", "shfmt"])

    def test_a_current_install_passes(self):
        self.install(self.current())
        self.assertEqual(lint.missing_tools(self.requirements, self.tools), [])

    def test_every_pin_is_exact_and_hashed(self):
        text = lint.REQUIREMENTS.read_text()
        entries = re.findall(r"(?m)^([a-z0-9-]+)==(\S+)((?: \\\n    --hash=sha256:[0-9a-f]{64})+)$", text)
        self.assertEqual({name for name, _, _ in entries}, {"ruff", "shellcheck-py", "shfmt-py"})
        requirements = [line for line in text.splitlines() if line and not line.startswith(("#", " "))]
        self.assertEqual(len(requirements), len(entries))


FILES = {"swift": ["A.swift"], "python": ["b.py"], "shell": ["c.sh"]}


class Commands(unittest.TestCase):
    def test_checks_change_no_file(self):
        for name, command in lint.commands(FILES, pinned_only=False):
            with self.subTest(tool=name):
                for writing in ("--in-place", "--write", "-w", "--fix"):
                    self.assertNotIn(writing, command)

    def test_pinned_only_leaves_out_the_xcode_formatter(self):
        names = [name for name, _ in lint.commands(FILES, pinned_only=True)]
        self.assertEqual(names, ["ruff format", "ruff check", "shfmt", "shellcheck"])
        self.assertIn("swift-format", [name for name, _ in lint.commands(FILES, pinned_only=False)])

    def test_check_and_format_use_one_shell_style(self):
        shfmt = dict(lint.commands(FILES, pinned_only=True))["shfmt"]
        self.assertEqual(shfmt[1 : 1 + len(lint.SHFMT_STYLE)], lint.SHFMT_STYLE)


class Registration(unittest.TestCase):
    def test_make_verify_runs_the_check_with_the_tools_as_a_preflight(self):
        policy = json.loads((ROOT / "scripts/verification.json").read_text())
        check = next(check for check in policy["checks"] if check["id"] == "check-lint")
        self.assertEqual((check["mode"], check.get("always")), ("automatic", True))
        self.assertIn("lint-tools", check["preflight"])
        self.assertEqual(policy["preflights"]["lint-tools"]["remedy"], "make setup-lint")

    def test_make_targets_share_the_script(self):
        def recipe(target):
            return subprocess.run(["make", "-n", target], cwd=ROOT, capture_output=True, text=True, check=True).stdout

        self.assertIn("scripts/lint.py check\n", recipe("check-lint"))
        self.assertIn("scripts/lint.py check --pinned-only", recipe("check-portable"))
        self.assertIn("scripts/lint.py format", recipe("fmt"))
        self.assertIn("scripts/lint.py setup", recipe("setup-lint"))
        setup = (ROOT / "Makefile").read_text().split("\nsetup:", 1)[1].split("\n\n", 1)[0]
        self.assertIn("setup-lint", setup)

    def test_public_ci_installs_the_pinned_tools_before_the_portable_checks(self):
        workflow = (ROOT / ".github/workflows/check.yml").read_text()
        self.assertLess(workflow.index("run: make setup-lint"), workflow.index("run: make check-portable"))


if __name__ == "__main__":
    unittest.main()
