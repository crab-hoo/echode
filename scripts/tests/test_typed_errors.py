"""ADR 0291: crates with typed errors do not return `String` errors again."""

from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

import check_typed_errors  # noqa: E402


class TypedErrors(unittest.TestCase):
    def tree(self, files):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        directory = Path(temporary.name)
        for crate in check_typed_errors.TYPED_CRATES:
            source = directory / "crates" / crate / "src"
            source.mkdir(parents=True)
            (source / "lib.rs").write_text("pub fn ok() -> Result<(), Error> { Ok(()) }\n")
        for relative, text in files.items():
            path = directory / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        return directory

    def test_the_repository_passes(self):
        self.assertEqual(check_typed_errors.problems(ROOT), [])

    def test_string_errors_in_any_position_are_found(self):
        crate = check_typed_errors.TYPED_CRATES[0]
        source = f"crates/{crate}/src/lib.rs"
        cases = {
            "pub fn f() -> Result<(), String> {}\n": [1],
            "fn f() -> Result<Vec<(u32, bool)>, std::string::String> {}\n": [1],
            "fn f() -> Result<\n    Option<u8>,\n    String,\n> {}\n": [1],
            "struct S { r: Receiver<Result<Prepared, String>> }\n": [1],
            "fn f(q: impl FnOnce(u32) -> Result<*mut u8, String>) {}\n": [1],
            "fn f() -> Result<Box<dyn Fn() -> u8>, String> {}\n": [1],
            "fn f() -> Result<String, Error> {}\n": [],
            "fn f() -> std::io::Result<String> {}\n": [],
            "// fn f() -> Result<(), String>\nfn g() {}\n": [],
            "fn g() {}\n#[cfg(test)]\nmod tests {\n    fn f() -> Result<(), String> {}\n}\n": [],
        }
        for text, lines in cases.items():
            with self.subTest(text=text):
                root = self.tree({source: text})
                self.assertEqual(
                    check_typed_errors.problems(root),
                    [f"{source}:{line}: returns a String error" for line in lines],
                )

    def test_test_files_are_fixtures(self):
        crate = check_typed_errors.TYPED_CRATES[0]
        root = self.tree({
            f"crates/{crate}/src/session_tests.rs": "fn f() -> Result<(), String> {}\n",
        })
        self.assertEqual(check_typed_errors.problems(root), [])

    def test_a_missing_crate_is_reported(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        crate = check_typed_errors.TYPED_CRATES[0]
        self.assertIn(
            f"crates/{crate}/src: missing",
            check_typed_errors.problems(Path(temporary.name)),
        )


if __name__ == "__main__":
    unittest.main()
