"""ADR 0321: every workspace crate keeps the forbidden undocumented `unsafe`."""

from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

import check_workspace_lints  # noqa: E402

LINTS = """
[workspace.lints.rust]
unsafe_op_in_unsafe_fn = "deny"

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
undocumented_unsafe_blocks = "forbid"
"""
INHERITS = "[package]\nname = \"x\"\n\n[lints]\nworkspace = true\n"


class WorkspaceLints(unittest.TestCase):
    def tree(self, lints=LINTS, crates=None):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        crates = {"crates/a": INHERITS, "spikes/b": INHERITS} if crates is None else crates
        listed = ", ".join(f'"{member}"' for member in crates)
        (root / "Cargo.toml").write_text(f"[workspace]\nmembers = [{listed}]\n{lints}")
        for member, manifest in crates.items():
            (root / member / "src").mkdir(parents=True)
            if manifest is not None:
                (root / member / "Cargo.toml").write_text(manifest)
        return root

    def test_the_repository_passes(self):
        self.assertEqual(check_workspace_lints.problems(ROOT), [])

    def test_a_crate_without_the_workspace_lints_is_found(self):
        cases = {
            "[package]\nname = \"x\"\n": "does not inherit the workspace lints",
            "[package]\nname = \"x\"\n\n[lints]\nworkspace = false\n": "does not inherit the workspace lints",
            "[package]\nname = \"x\"\n\n[lints.clippy]\nall = \"warn\"\n": "does not inherit the workspace lints",
            "[package\n": "unreadable",
        }
        for manifest, problem in cases.items():
            with self.subTest(manifest=manifest):
                found = check_workspace_lints.problems(self.tree(crates={"crates/a": INHERITS, "crates/new": manifest}))
                self.assertEqual(len(found), 1, found)
                self.assertTrue(found[0].startswith("crates/new/Cargo.toml: " + problem), found)

    def test_a_missing_member_manifest_is_found(self):
        found = check_workspace_lints.problems(self.tree(crates={"crates/a": INHERITS, "crates/new": None}))
        self.assertEqual(len(found), 1, found)
        self.assertTrue(found[0].startswith("crates/new/Cargo.toml: unreadable"), found)

    def test_a_weaker_workspace_level_is_found(self):
        cases = {
            LINTS.replace('undocumented_unsafe_blocks = "forbid"', 'undocumented_unsafe_blocks = "deny"'):
                "Cargo.toml: clippy::undocumented_unsafe_blocks is deny, not forbid",
            LINTS.replace('undocumented_unsafe_blocks = "forbid"\n', ""):
                "Cargo.toml: clippy::undocumented_unsafe_blocks is unset, not forbid",
            LINTS.replace('unsafe_op_in_unsafe_fn = "deny"', 'unsafe_op_in_unsafe_fn = "warn"'):
                "Cargo.toml: rust::unsafe_op_in_unsafe_fn is warn, not deny or forbid",
        }
        for lints, problem in cases.items():
            with self.subTest(problem=problem):
                self.assertEqual(check_workspace_lints.problems(self.tree(lints=lints)), [problem])
        table = LINTS.replace('undocumented_unsafe_blocks = "forbid"',
                              'undocumented_unsafe_blocks = { level = "forbid", priority = 1 }')
        self.assertEqual(check_workspace_lints.problems(self.tree(lints=table)), [])


if __name__ == "__main__":
    unittest.main()
