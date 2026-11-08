"""ADR 0284: the audio-path crates keep denying panicking lints."""

from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

import check_audio_lints  # noqa: E402

DENY = """#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic
    )
)]
"""


class AudioLints(unittest.TestCase):
    def tree(self, roots, extra=None):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        directory = Path(temporary.name)
        for crate in check_audio_lints.AUDIO_CRATES:
            source = directory / "crates" / crate / "src"
            source.mkdir(parents=True)
            (source / "lib.rs").write_text(roots.get(crate, DENY))
        for name, text in (extra or {}).items():
            (directory / name).write_text(text)
        return directory

    def test_the_repository_passes(self):
        self.assertEqual(check_audio_lints.problems(ROOT), [])

    def test_a_root_without_the_attribute_or_with_a_lint_missing_is_refused(self):
        weaker = DENY.replace("        clippy::panic\n", "")
        found = check_audio_lints.problems(self.tree({
            "echode-engine": "//! Engine.\n",
            "echode-realtime": weaker,
        }))
        self.assertEqual(len(found), 2, found)
        self.assertIn("crates/echode-engine/src/lib.rs", found[0] + found[1])
        self.assertIn("crates/echode-realtime/src/lib.rs", found[0] + found[1])

    def test_a_local_allow_or_expect_is_refused_but_an_expect_call_is_not(self):
        found = check_audio_lints.problems(self.tree({}, {
            "crates/echode-engine/src/mix.rs":
                "#[allow(clippy::indexing_slicing)]\nfn f() {}\n"
                "#[expect(clippy::unwrap_used, reason = \"x\")]\nfn g() {}\n"
                "fn h(x: Option<u8>) -> u8 { x.expect(\"clippy::panic\") }\n",
            "crates/echode-engine/src/graph.rs": "#![allow(clippy::indexing_slicing)]\n",
        }))
        self.assertEqual(found, [
            "crates/echode-engine/src/mix.rs:1: allows clippy::indexing_slicing",
            "crates/echode-engine/src/mix.rs:3: allows clippy::unwrap_used",
        ])


if __name__ == "__main__":
    unittest.main()
