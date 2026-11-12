"""ADR 0291, 0294, 0320: workspace crates do not return `String` errors again."""

import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

import check_typed_errors

CRATE = "crates/echode-audio-io"
# A sample of the workspace's shapes: product crates, the history crate with
# its own package rule, and a spike member outside `crates/`.
MEMBERS = (
    CRATE,
    "crates/echode-history",
    "crates/echode-sandbox",
    "crates/echode-dsp",
    "spikes/bridge",
)


class TypedErrors(unittest.TestCase):
    def tree(self, files, members=MEMBERS, manifest=None):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        directory = Path(temporary.name)
        if manifest is None:
            listed = ", ".join(f'"{member}"' for member in members)
            manifest = f'[workspace]\nresolver = "3"\nmembers = [{listed}]\n'
        if manifest is not False:
            (directory / "Cargo.toml").write_text(manifest)
        for member in members:
            if "*" in member:
                continue
            source = directory / member / "src"
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
        source = f"{CRATE}/src/lib.rs"
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

    def test_errors_turned_into_text_are_found(self):
        source = f"{CRATE}/src/lib.rs"
        cases = {
            "fn f() { g().map_err(|error| E::Other(error.to_string()))?; }\n": [1],
            'fn f() {\n    g()\n        .map_err(|status| E::Io(format!("x: {status}")))?;\n}\n': [3],
            "fn f() { g().map_err(|e| E::A(Box::new(e.to_string ())))?; }\n": [1],
            "fn f() { g().map_err(E::Io)?; let s = x.to_string(); }\n": [],
            "fn f() { g().map_err(|error| E::Io(Arc::new(error)))?; }\n": [],
            "// g().map_err(|e| e.to_string())\nfn f() {}\n": [],
        }
        for text, lines in cases.items():
            with self.subTest(text=text):
                root = self.tree({source: text})
                self.assertEqual(
                    check_typed_errors.problems(root),
                    [f"{source}:{line}: turns an error into text" for line in lines],
                )

    def test_io_error_constructors_do_not_erase_causes_outside_map_err(self):
        source = f"{CRATE}/src/lib.rs"
        cases = {
            'fn f() { io::Error::other(format!("shm_open: {}", io::Error::last_os_error())) }\n': [1],
            "fn f() { std::io::Error::new(ErrorKind::Other, error.to_string()) }\n": [1],
            'fn f() { io::Error::new(ErrorKind::Other, format!("shm_unlink: {}", io::Error::last_os_error())) }\n': [1],
            'fn f() { io::Error::new(ErrorKind::InvalidInput, format!("{} {reason}", name.to_string_lossy())) }\n': [1],
            'fn f() { io::Error::new(ErrorKind::InvalidData, format!("bad {field}")) }\n': [1],
            'fn f() { io::Error\n    ::other(\n        format!("ftruncate: {error}")\n    ) }\n': [1],
            "fn f() { io::Error::other(error) }\n": [],
            "fn f() { io::Error::new(ErrorKind::InvalidData, error) }\n": [],
            '// io::Error::other(format!("{error}"))\nfn f() {}\n': [],
        }
        for text, lines in cases.items():
            with self.subTest(text=text):
                root = self.tree({source: text})
                self.assertEqual(
                    check_typed_errors.problems(root),
                    [f"{source}:{line}: wraps error text in an I/O error" for line in lines],
                )

    def test_literals_need_a_typed_cause_in_every_member(self):
        cases = {
            'fn f() { io::Error::new(ErrorKind::InvalidData, "invalid frame") }\n': [1],
            'fn f() { io::Error::new(\n    ErrorKind::InvalidInput,\n    "remap",\n) }\n': [1],
            'fn f() { std::io::Error::new(std::io::ErrorKind::Other, r"raw") }\n': [1],
            'fn f() { io::Error::other("constant") }\n': [1],
            'fn f() { std::io::Error::new(std::io::ErrorKind::NotFound, "media is missing") }\n': [1],
            "fn f() { io::Error::new(ErrorKind::InvalidData, error) }\n": [],
            'fn f() { io::Error::new(ErrorKind::InvalidData, f(a, "b")) }\n': [],
            'fn f() { io::Error::new(\n    ErrorKind::TimedOut,\n    "deadline",\n) }\n': [],
            'fn f() { std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "closed") }\n': [],
            'fn f() { io::Error::new(ErrorKind::WriteZero, "no bytes") }\n': [],
            '// io::Error::other("constant")\nfn f() {}\n': [],
        }
        for member in MEMBERS:
            source = f"{member}/src/control.rs"
            for text, lines in cases.items():
                with self.subTest(source=source, text=text):
                    root = self.tree({source: text})
                    self.assertEqual(
                        check_typed_errors.problems(root),
                        [f"{source}:{line}: gives an I/O error only constant text" for line in lines],
                    )

    def test_migrated_package_modules_reject_untyped_messages(self):
        cases = [
            'fn f() { PackageError::new("no samples") }\n',
            "fn f() { PackageError::Message(message) }\n",
            'fn f() { PackageError\n    ::new("no samples") }\n',
        ]
        for source in ("crates/echode-history/src/store.rs", "crates/echode-history/src/new.rs"):
            for text in cases:
                with self.subTest(source=source, text=text):
                    root = self.tree({source: text})
                    self.assertEqual(
                        check_typed_errors.problems(root),
                        [
                            f"{source}:1: returns an untyped package message",
                        ],
                    )
            root = self.tree({source: "fn f() { PackageError::Audio(MediaError::NoSamples) }\n"})
            self.assertEqual(check_typed_errors.problems(root), [])

    def test_migrated_package_modules_reject_string_results_and_erased_causes(self):
        cases = [
            ("fn f() -> Result<(), String> {}\n", "returns a String error"),
            ("fn f() { g().map_err(|e| E::Other(e.to_string()))?; }\n", "turns an error into text"),
        ]
        for source in ("crates/echode-history/src/format.rs", "crates/echode-history/src/new.rs"):
            for text, problem in cases:
                with self.subTest(source=source, text=text):
                    root = self.tree({source: text})
                    self.assertEqual(
                        check_typed_errors.problems(root),
                        [
                            f"{source}:1: {problem}",
                        ],
                    )

    def test_every_member_guards_existing_new_and_nested_modules(self):
        cases = [
            ("fn f() -> Result<(), String> {}\n", "returns a String error"),
            ("fn f() { g().map_err(|e| E::Io(e.to_string()))?; }\n", "turns an error into text"),
        ]
        for member in MEMBERS:
            for name in ("lib.rs", "new.rs", "nested/new.rs"):
                source = f"{member}/src/{name}"
                for text, problem in cases:
                    with self.subTest(source=source, text=text):
                        root = self.tree({source: text})
                        self.assertEqual(
                            check_typed_errors.problems(root),
                            [
                                f"{source}:1: {problem}",
                            ],
                        )

    def test_a_new_workspace_member_is_checked_without_a_list(self):
        source = "crates/echode-new/src/lib.rs"
        root = self.tree(
            {source: "fn f() -> Result<(), String> {}\n"},
            members=MEMBERS + ("crates/echode-new",),
        )
        self.assertEqual(
            check_typed_errors.problems(root),
            [
                f"{source}:1: returns a String error",
            ],
        )

    def test_glob_members_are_expanded(self):
        source = "crates/echode-new/src/lib.rs"
        manifest = '[workspace]\nmembers = ["crates/*"]\n'
        root = self.tree({source: "fn f() -> Result<(), String> {}\n"}, manifest=manifest)
        self.assertEqual(
            check_typed_errors.problems(root),
            [
                f"{source}:1: returns a String error",
            ],
        )

    def test_folders_outside_the_workspace_are_not_checked(self):
        root = self.tree(
            {
                "spikes/draw/src/lib.rs": "fn f() -> Result<(), String> {}\n",
            }
        )
        self.assertEqual(check_typed_errors.problems(root), [])

    def test_test_files_are_fixtures(self):
        root = self.tree(
            {
                f"{CRATE}/src/session_tests.rs": "fn f() -> Result<(), String> {}\n",
                f"{CRATE}/src/tests/fixture.rs": "fn f() -> Result<(), String> {}\n",
            }
        )
        self.assertEqual(check_typed_errors.problems(root), [])

    def test_a_member_without_sources_is_reported(self):
        root = self.tree({})
        for item in sorted((root / CRATE / "src").iterdir()):
            item.unlink()
        (root / CRATE / "src").rmdir()
        self.assertEqual(check_typed_errors.problems(root), [f"{CRATE}/src: missing"])

    def test_a_missing_manifest_or_member_list_is_reported(self):
        root = self.tree({}, manifest=False)
        self.assertEqual(check_typed_errors.problems(root), ["Cargo.toml: missing"])
        root = self.tree({}, manifest='[package]\nname = "x"\n')
        self.assertEqual(len(check_typed_errors.problems(root)), 1)
        self.assertTrue(check_typed_errors.problems(root)[0].startswith("Cargo.toml: no workspace members"))
        root = self.tree({}, manifest="[workspace]\nmembers = []\n")
        self.assertEqual(check_typed_errors.problems(root), ["Cargo.toml: no workspace members"])
        root = self.tree({}, manifest='[workspace]\nmembers = ["crates/gone"]\n')
        self.assertEqual(check_typed_errors.problems(root), ["crates/gone/src: missing"])
        root = self.tree({}, manifest='[workspace]\nmembers = ["gone/*"]\n')
        self.assertEqual(check_typed_errors.problems(root), ["gone/*: missing"])


if __name__ == "__main__":
    unittest.main()
