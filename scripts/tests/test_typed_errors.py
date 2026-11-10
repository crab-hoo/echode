"""ADR 0291, 0294: crates with typed errors do not return `String` errors again."""

from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

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
        for relative in check_typed_errors.TYPED_FILES:
            source = directory / relative
            source.parent.mkdir(parents=True, exist_ok=True)
            source.write_text("pub fn ok() -> Result<(), Error> { Ok(()) }\n")
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

    def test_errors_turned_into_text_are_found(self):
        crate = check_typed_errors.TYPED_CRATES[0]
        source = f"crates/{crate}/src/lib.rs"
        cases = {
            "fn f() { g().map_err(|error| E::Other(error.to_string()))?; }\n": [1],
            "fn f() {\n    g()\n        .map_err(|status| E::Io(format!(\"x: {status}\")))?;\n}\n": [3],
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

    def test_migrated_package_modules_reject_untyped_messages(self):
        cases = [
            'fn f() { PackageError::new("no samples") }\n',
            'fn f() { PackageError::Message(message) }\n',
            'fn f() { PackageError\n    ::new("no samples") }\n',
        ]
        for source in ('crates/echode-history/src/store.rs', 'crates/echode-history/src/new.rs'):
            for text in cases:
                with self.subTest(source=source, text=text):
                    root = self.tree({source: text})
                    self.assertEqual(check_typed_errors.problems(root), [
                        f"{source}:1: returns an untyped package message",
                    ])
            root = self.tree({source: 'fn f() { PackageError::Audio(MediaError::NoSamples) }\n'})
            self.assertEqual(check_typed_errors.problems(root), [])

    def test_migrated_package_modules_reject_string_results_and_erased_causes(self):
        cases = [
            ('fn f() -> Result<(), String> {}\n', 'returns a String error'),
            ('fn f() { g().map_err(|e| E::Other(e.to_string()))?; }\n', 'turns an error into text'),
        ]
        for source in ('crates/echode-history/src/format.rs', 'crates/echode-history/src/new.rs'):
            for text, problem in cases:
                with self.subTest(source=source, text=text):
                    root = self.tree({source: text})
                    self.assertEqual(check_typed_errors.problems(root), [
                        f"{source}:1: {problem}",
                    ])

    def test_completed_history_crate_guards_new_and_spike_modules(self):
        for name in ('store.rs', 'format.rs', 'new_module.rs', 'nested/new_module.rs'):
            source = f'crates/echode-history/src/{name}'
            with self.subTest(source=source):
                root = self.tree({source: 'fn f() -> Result<(), String> {}\n'})
                self.assertEqual(check_typed_errors.problems(root), [
                    f'{source}:1: returns a String error',
                ])

    def test_completed_ffi_crate_guards_catalog_and_new_modules(self):
        cases = [
            ('fn f() -> Result<(), String> {}\n', 'returns a String error'),
            ('fn f() { g().map_err(|e| e.to_string())?; }\n', 'turns an error into text'),
        ]
        for name in ('catalog.rs', 'new.rs', 'nested/new.rs'):
            source = f'crates/echode-ffi/src/{name}'
            for text, problem in cases:
                with self.subTest(source=source, text=text):
                    root = self.tree({source: text})
                    self.assertEqual(check_typed_errors.problems(root), [
                        f'{source}:1: {problem}',
                    ])

    def test_completed_model_crate_guards_validation_and_new_modules(self):
        cases = [
            ('fn f() -> Result<(), String> {}\n', 'returns a String error'),
            ('fn f() { g().map_err(|e| E::Invariant(e.to_string()))?; }\n', 'turns an error into text'),
        ]
        for name in ('model.rs', 'invariants.rs', 'error.rs', 'new.rs', 'nested/new.rs'):
            source = f'crates/echode-model/src/{name}'
            for text, problem in cases:
                with self.subTest(source=source, text=text):
                    root = self.tree({source: text})
                    self.assertEqual(check_typed_errors.problems(root), [
                        f'{source}:1: {problem}',
                    ])

    def test_completed_api_crate_guards_transport_rpc_and_new_modules(self):
        cases = [
            ('fn f() -> Result<(), String> {}\n', 'returns a String error'),
            ('fn f() { g().map_err(|e| E::Io(e.to_string()))?; }\n', 'turns an error into text'),
        ]
        for name in ('serve.rs', 'mcp.rs', 'rpc.rs', 'error.rs', 'new.rs', 'nested/new.rs'):
            source = f'crates/echode-api/src/{name}'
            for text, problem in cases:
                with self.subTest(source=source, text=text):
                    root = self.tree({source: text})
                    self.assertEqual(check_typed_errors.problems(root), [
                        f'{source}:1: {problem}',
                    ])

    def test_completed_cli_crate_guards_render_io_and_new_modules(self):
        cases = [
            ('fn f() -> Result<(), String> {}\n', 'returns a String error'),
            ('fn f() { g().map_err(|e| E::Io(e.to_string()))?; }\n', 'turns an error into text'),
        ]
        for name in ('lib.rs', 'destination.rs', 'normalization.rs', 'error.rs', 'new.rs', 'nested/new.rs'):
            source = f'crates/echode-cli/src/{name}'
            for text, problem in cases:
                with self.subTest(source=source, text=text):
                    root = self.tree({source: text})
                    self.assertEqual(check_typed_errors.problems(root), [
                        f'{source}:1: {problem}',
                    ])

    def test_completed_actions_crate_guards_decoding_and_new_modules(self):
        cases = [
            ('fn f() -> Result<(), String> {}\n', 'returns a String error'),
            ('fn f() { g().map_err(|e| E::Json(e.to_string()))?; }\n', 'turns an error into text'),
        ]
        for name in ('action.rs', 'parameters.rs', 'new.rs', 'nested/new.rs'):
            source = f'crates/echode-actions/src/{name}'
            for text, problem in cases:
                with self.subTest(source=source, text=text):
                    root = self.tree({source: text})
                    self.assertEqual(check_typed_errors.problems(root), [
                        f'{source}:1: {problem}',
                    ])

    def test_unmigrated_crates_are_not_claimed_as_typed(self):
        root = self.tree({
            'crates/unmigrated/src/store.rs': 'fn f() -> Result<(), String> {}\n',
        })
        self.assertEqual(check_typed_errors.problems(root), [])

    def test_a_missing_migrated_file_is_reported(self):
        source = 'crates/unmigrated/src/typed.rs'
        with patch.object(check_typed_errors, 'TYPED_FILES', (source,)):
            root = self.tree({})
            (root / source).unlink()
            self.assertEqual(check_typed_errors.problems(root), [f"{source}: missing"])

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
