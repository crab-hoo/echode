"""The durability trace analysis, registered audit reproductions, and foreign declarations."""

import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import check_ffi
import durability_trace
import verify


class DurabilityTraceTests(unittest.TestCase):
    def test_a_rename_needs_its_directory_synced_before_the_next_journal_event(self):
        unsynced = [
            ("RENAME", ".git/objects/ab/cd"),
            ("RENAME", "media/objects/h"),
            ("SYNC", "media/objects"),
            ("RENAME", ".journal.jsonl.1.tmp"),
        ]
        self.assertEqual(durability_trace.unsynced(unsynced), [".git/objects/ab/cd"])
        synced = [
            ("RENAME", ".git/refs/echode/autosave/main"),
            ("SYNC", ".git/refs/echode/autosave"),
            ("SYNC", "journal.jsonl"),
        ]
        self.assertEqual(durability_trace.unsynced(synced), [])

    def test_events_keep_only_paths_inside_the_package(self):
        package = Path(tempfile.mkdtemp())
        text = f"SYNC {package}\nRENAME {package}/media/objects/h\nSYNC /elsewhere/file\n"
        self.assertEqual(durability_trace.events(text, package), [("SYNC", "."), ("RENAME", "media/objects/h")])


class RegisteredAuditReproductions(unittest.TestCase):
    """Reproductions from docs/reviews/2026-11-02-fix-instructions.md, fixed by ADR 0232."""

    def test_audit_env01_deleting_unsafe_code_selects_the_sanitizers(self):
        directory = Path(tempfile.mkdtemp())

        def git(*arguments):
            subprocess.run(["git", *arguments], cwd=directory, check=True, capture_output=True)

        git("init", "-q")
        git("config", "user.email", "audit@example.invalid")
        git("config", "user.name", "audit")
        (directory / "crates/x/src").mkdir(parents=True)
        (directory / "crates/x/src/a.rs").write_text("pub fn f() { unsafe {} }\n")
        git("add", ".")
        git("commit", "-qm", "base")
        os.unlink(directory / "crates/x/src/a.rs")
        policy = {"checks": [{"id": "san", "task_unsafe": True, "inputs": ["crates/**"], "packages": []}]}
        found = verify.unsafe_changes(directory, policy, verify.changed_paths(directory))
        self.assertEqual(found, {"san": ["crates/x/src/a.rs"]})

    def test_audit_env06_the_command_line_cannot_satisfy_an_expected_failure(self):
        directory = Path(tempfile.mkdtemp())
        command = f"{sys.executable} -c 'import sys; sys.exit(101)' the_regression_assertion"
        result = subprocess.run(
            [
                sys.executable,
                "-B",
                str(ROOT / "scripts/checked_commands.py"),
                "--log-dir",
                str(directory),
                "--command",
                command,
                "--expect-exit",
                "101",
                "--expect-failure",
                "the_regression_assertion",
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(result.returncode, 0, "text only in the Command: line passed as the expected red")

    def test_a_changed_c_line_selects_the_sanitizers_and_tests_do_not(self):
        directory = Path(tempfile.mkdtemp())

        def git(*arguments):
            subprocess.run(["git", *arguments], cwd=directory, check=True, capture_output=True)

        git("init", "-q")
        git("config", "user.email", "audit@example.invalid")
        git("config", "user.name", "audit")
        (directory / "bridge").mkdir()
        (directory / "bridge/tests").mkdir()
        for name in ("bridge/shim.c", "bridge/shim.h", "bridge/tests/fixture.c"):
            (directory / name).write_text("int f(void) { return 0; }\n")
        git("add", ".")
        git("commit", "-qm", "base")
        for name in ("bridge/shim.c", "bridge/shim.h", "bridge/tests/fixture.c"):
            (directory / name).write_text("int f(void) { return 1; }\n")
        policy = {"checks": [{"id": "san", "task_unsafe": True, "inputs": ["bridge/**"], "packages": []}]}
        found = verify.unsafe_changes(directory, policy, verify.changed_paths(directory))
        self.assertEqual(found, {"san": ["bridge/shim.c", "bridge/shim.h"]})


class ForeignDeclarationTree(unittest.TestCase):
    """Runs the real check-ffi entry point on a small Rust tree."""

    def check_sources(self, sources, allowlist, deployment_target="27.0"):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / "scripts").mkdir()
            for name in ("check-ffi.sh", "check_ffi.py"):
                source = ROOT / "scripts" / name
                if source.exists():
                    (directory / "scripts" / name).write_text(source.read_text())
            (directory / "scripts/build-platform.env").write_text(
                f"export MACOSX_DEPLOYMENT_TARGET={deployment_target}\n"
            )
            (directory / "scripts/ffi-allowlist.txt").write_text(allowlist)
            (directory / "crates/x/src").mkdir(parents=True)
            for name, text in sources.items():
                (directory / "crates/x/src" / name).write_text(text)
            return subprocess.run(
                ["bash", str(directory / "scripts/check-ffi.sh")], capture_output=True, text=True, check=False
            )


class ForeignDeclarations(ForeignDeclarationTree):
    """ENV-05 / FCA-12: foreign declarations are checked by path and symbol."""

    def test_extern_blocks_without_an_abi_with_another_abi_or_a_split_brace_are_found(self):
        result = self.check_sources(
            {
                "listed.rs": 'extern "C" {\n    fn os_release(object: *mut std::ffi::c_void);\n}\n',
                "plain.rs": "unsafe extern {\n    fn f();\n}\n",
                "system.rs": 'extern "system" {\n    fn f();\n}\n',
                "split.rs": 'unsafe extern "C"\n{\n    fn f();\n}\n',
                "callback.rs": 'pub extern "C" fn f() {}\nextern crate core;\n',
            },
            "crates/x/src/listed.rs os_release os/object.h\n",
        )
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        for name in ("plain.rs", "system.rs", "split.rs"):
            self.assertIn("crates/x/src/" + name, result.stdout)
        self.assertNotIn("callback.rs", result.stdout, "a definition is not a declaration")

    def test_a_new_symbol_in_a_listed_file_is_rejected(self):
        result = self.check_sources(
            {
                "listed.rs": 'unsafe extern "C" { fn f(); fn added(); }\n',
            },
            "crates/x/src/listed.rs f os/object.h\n",
        )
        self.assertEqual(result.returncode, 1, "an added foreign symbol bypassed the allowlist")
        self.assertIn("added", result.stdout)

    def test_stale_symbols_are_rejected_even_when_the_file_still_declares_others(self):
        result = self.check_sources(
            {
                "listed.rs": 'extern "C" { fn f(); }\n',
            },
            "crates/x/src/listed.rs f os/object.h\ncrates/x/src/listed.rs removed os/object.h\n",
        )
        self.assertEqual(result.returncode, 1, "a stale foreign symbol remained allowed")
        self.assertIn("removed", result.stdout)

    def test_static_items_comments_and_strings_are_distinguished(self):
        result = self.check_sources(
            {
                "listed.rs": """
                // extern "C" { fn imaginary(); }
                /* nested /* extern { fn fake(); } */ comment */
                const EXAMPLE: &str = r#"extern { fn quoted(); }"#;
                extern "C" {
                    pub(crate) static bootstrap_port: u32;
                    fn os_release(object: *mut std::ffi::c_void);
                }
            """,
            },
            "crates/x/src/listed.rs bootstrap_port servers/bootstrap.h\n"
            "crates/x/src/listed.rs os_release os/object.h\n",
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_static_mut_raw_identifiers_and_callback_types_are_named(self):
        source = 'extern "C" { static mut port: u32; fn f(callback: unsafe extern "C" fn(i32)) -> i32; fn r#type(); }'
        self.assertEqual(check_ffi.declarations(source), {"port", "f", "type"})

    def test_an_empty_allowlist_and_foreign_free_sources_are_valid(self):
        result = self.check_sources({"local.rs": 'pub extern "C" fn f() {}\n'}, "# no exceptions\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_a_link_name_cannot_redirect_an_allowed_declaration(self):
        result = self.check_sources(
            {
                "listed.rs": 'extern "C" { #[link_name = "unexpected"] fn f(); }\n',
            },
            "crates/x/src/listed.rs f os/object.h\n",
        )
        self.assertEqual(result.returncode, 1, "an allowed name concealed another foreign symbol")
        self.assertIn("link_name", result.stdout)

    def test_an_allowlist_entry_names_the_header_that_declares_it(self):
        result = self.check_sources(
            {"listed.rs": 'extern "C" { fn os_release(object: *mut std::ffi::c_void); }\n'},
            "crates/x/src/listed.rs os_release\n",
        )
        self.assertEqual(result.returncode, 1, "an entry without its SDK header was accepted")
        self.assertIn("os_release", result.stdout + result.stderr)


WORKGROUP_DECLARATIONS = """
use std::ffi::{c_char, c_int, c_void};
use std::marker::{PhantomData, PhantomPinned};

/// Only used behind a pointer.
#[repr(C)]
pub struct os_workgroup_s {
    _opaque: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

#[repr(C)]
pub struct os_workgroup_join_token_opaque_s {
    pub sig: u32,
    pub opaque: [c_char; 36],
}

#[link(name = "System")]
unsafe extern "C" {
    pub fn os_workgroup_create_with_port(name: *const c_char, mach_port: u32) -> *mut os_workgroup_s;
    pub fn os_workgroup_join(wg: *mut os_workgroup_s, token_out: *mut os_workgroup_join_token_opaque_s) -> c_int;
    pub fn os_release(object: *mut c_void);
    pub static bootstrap_port: u32;
}
"""

WORKGROUP_ALLOWLIST = (
    "crates/x/src/listed.rs os_workgroup_create_with_port os/workgroup.h\n"
    "crates/x/src/listed.rs os_workgroup_join os/workgroup.h\n"
    "crates/x/src/listed.rs os_release os/object.h\n"
    "crates/x/src/listed.rs bootstrap_port servers/bootstrap.h\n"
)


class ForeignDeclarationsMatchTheSdk(ForeignDeclarationTree):
    """ENG-15: each remaining handwritten declaration is compiled against the macOS SDK."""

    def check_changed(self, old, new, allowlist=WORKGROUP_ALLOWLIST, deployment_target="27.0"):
        self.assertIn(old, WORKGROUP_DECLARATIONS)
        return self.check_sources({"listed.rs": WORKGROUP_DECLARATIONS.replace(old, new)}, allowlist, deployment_target)

    def assertRejected(self, result, *needles):
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("differ from the macOS SDK", result.stdout, result.stdout + result.stderr)
        for needle in needles:
            self.assertIn(needle, result.stdout, result.stdout + result.stderr)

    def test_declarations_that_match_the_sdk_pass(self):
        result = self.check_sources({"listed.rs": WORKGROUP_DECLARATIONS}, WORKGROUP_ALLOWLIST)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_a_parameter_of_another_type_is_rejected(self):
        result = self.check_changed("os_release(object: *mut c_void)", "os_release(object: *mut u32)")
        self.assertRejected(result, "crates/x/src/listed.rs os_release")

    def test_a_pointee_that_loses_const_is_rejected(self):
        result = self.check_changed("name: *const c_char", "name: *mut c_char")
        self.assertRejected(result, "crates/x/src/listed.rs os_workgroup_create_with_port")

    def test_a_return_of_another_sign_is_rejected(self):
        result = self.check_changed(
            "token_out: *mut os_workgroup_join_token_opaque_s) -> c_int",
            "token_out: *mut os_workgroup_join_token_opaque_s) -> u32",
        )
        self.assertRejected(result, "crates/x/src/listed.rs os_workgroup_join")

    def test_a_static_of_another_type_is_rejected(self):
        result = self.check_changed("bootstrap_port: u32", "bootstrap_port: i32")
        self.assertRejected(result, "crates/x/src/listed.rs bootstrap_port")

    def test_a_struct_of_another_size_is_rejected(self):
        result = self.check_changed("[c_char; 36]", "[c_char; 32]")
        self.assertRejected(result, "struct os_workgroup_join_token_opaque_s")

    def test_a_struct_field_of_another_type_is_rejected(self):
        result = self.check_changed("[c_char; 36]", "[u8; 36]")
        self.assertRejected(result, "struct os_workgroup_join_token_opaque_s", "opaque")

    def test_a_struct_without_a_c_layout_is_rejected(self):
        result = self.check_changed(
            "#[repr(C)]\npub struct os_workgroup_join_token_opaque_s", "pub struct os_workgroup_join_token_opaque_s"
        )
        self.assertRejected(result, "os_workgroup_join_token_opaque_s", "repr(C)")

    def test_an_unsupported_rust_type_is_rejected(self):
        result = self.check_changed("os_release(object: *mut c_void)", "os_release(object: Option<&mut u8>)")
        self.assertRejected(result, "crates/x/src/listed.rs os_release", "unsupported")

    def test_a_deprecated_declaration_is_rejected(self):
        result = self.check_sources(
            {
                "listed.rs": "use std::ffi::{c_int, c_uint};\n"
                'extern "C" { fn sem_init(sem: *mut c_int, pshared: c_int, value: c_uint) -> c_int; }\n'
            },
            "crates/x/src/listed.rs sem_init semaphore.h\n",
        )
        self.assertRejected(result, "sem_init", "deprecated")

    def test_a_declaration_newer_than_the_deployment_target_is_rejected(self):
        result = self.check_sources({"listed.rs": WORKGROUP_DECLARATIONS}, WORKGROUP_ALLOWLIST, "10.15")
        self.assertRejected(result, "os_workgroup_join", "macOS 11.0")

    def test_a_header_that_does_not_declare_the_symbol_is_rejected(self):
        result = self.check_sources(
            {"listed.rs": WORKGROUP_DECLARATIONS},
            WORKGROUP_ALLOWLIST.replace("os_release os/object.h", "os_release stdint.h"),
        )
        self.assertRejected(result, "crates/x/src/listed.rs os_release", "undeclared")


if __name__ == "__main__":
    unittest.main()
