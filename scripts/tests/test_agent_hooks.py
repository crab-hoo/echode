"""Claude Code hook adapters, the durability trace analysis, and registered audit reproductions."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import check_on_stop  # noqa: E402
import durability_trace  # noqa: E402
import verify  # noqa: E402
import wait_for_verify  # noqa: E402


class ClaudeHookTests(unittest.TestCase):
    def test_settings_run_existing_adapters_within_the_policy_budget(self):
        settings = json.loads((ROOT / ".claude/settings.json").read_text())["hooks"]
        budget = verify.load_policy(ROOT)["hook_timeout"]
        self.assertEqual(set(settings), {"PreToolUse", "Stop"})
        for entries in settings.values():
            for entry in entries:
                for hook in entry["hooks"]:
                    self.assertEqual(hook["timeout"], budget)
                    script = ROOT / hook["command"].replace('"$CLAUDE_PROJECT_DIR"/', "")
                    self.assertTrue(os.access(script, os.X_OK), script)
        matcher = set(settings["PreToolUse"][0]["matcher"].split("|"))
        self.assertEqual(matcher, wait_for_verify.CLAUDE_MUTATING)

    def test_edits_wait_for_the_lock_and_answer_in_claude_format(self):
        root = Path(tempfile.mkdtemp())
        allowed = wait_for_verify.respond({"tool_name": "Edit"}, root, timeout=1,
                                          mutating=wait_for_verify.CLAUDE_MUTATING)
        self.assertEqual(wait_for_verify.claude_response(allowed), {})
        denied = wait_for_verify.claude_response({"permission": "deny", "agent_message": "busy"})
        self.assertEqual(denied["hookSpecificOutput"]["permissionDecision"], "deny")
        self.assertEqual(denied["hookSpecificOutput"]["permissionDecisionReason"], "busy")
        self.assertEqual(wait_for_verify.respond({"tool_name": "Bash"}, root, timeout=1,
                                                 mutating=wait_for_verify.CLAUDE_MUTATING),
                         {"permission": "allow"})

    def test_real_wrapper_allows_a_read_and_denies_invalid_input(self):
        script = ROOT / ".claude/hooks/wait-for-verify.sh"
        allowed = subprocess.run([str(script)], cwd=ROOT, input=json.dumps({"tool_name": "Read"}),
                                 capture_output=True, text=True, check=True)
        self.assertEqual(json.loads(allowed.stdout), {})
        denied = subprocess.run([str(script)], cwd=ROOT, input="{invalid",
                                capture_output=True, text=True, check=True)
        self.assertEqual(json.loads(denied.stdout)["hookSpecificOutput"]["permissionDecision"], "deny")

    def test_stop_blocks_a_failed_verification_a_bounded_number_of_times(self):
        counts = Path(tempfile.mkdtemp()) / "blocks.json"
        event = {"session_id": "s", "stop_hook_active": False}
        for _ in range(check_on_stop.LOOP_LIMIT):
            self.assertEqual(check_on_stop.respond_claude(event, lambda: 1, counts)["decision"], "block")
        final = check_on_stop.respond_claude(event, lambda: 1, counts)
        self.assertNotIn("decision", final)
        self.assertIn("systemMessage", final)
        self.assertEqual(check_on_stop.respond_claude(event, lambda: 1, counts)["decision"], "block")
        self.assertEqual(check_on_stop.respond_claude(event, lambda: 0, counts), {})
        self.assertEqual(json.loads(counts.read_text()), {})
        with self.assertRaises(ValueError):
            check_on_stop.respond_claude([], lambda: 0, counts)


class DurabilityTraceTests(unittest.TestCase):
    def test_a_rename_needs_its_directory_synced_before_the_next_journal_event(self):
        unsynced = [("RENAME", ".git/objects/ab/cd"), ("RENAME", "media/objects/h"),
                    ("SYNC", "media/objects"), ("RENAME", ".journal.jsonl.1.tmp")]
        self.assertEqual(durability_trace.unsynced(unsynced), [".git/objects/ab/cd"])
        synced = [("RENAME", ".git/refs/echode/autosave/main"), ("SYNC", ".git/refs/echode/autosave"),
                  ("SYNC", "journal.jsonl")]
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
        result = subprocess.run([sys.executable, "-B", str(ROOT / "scripts/checked_commands.py"),
                                 "--log-dir", str(directory), "--command", command, "--expect-exit", "101",
                                 "--expect-failure", "the_regression_assertion"], capture_output=True, text=True)
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


class ForeignDeclarations(unittest.TestCase):
    """ENV-05 / FCA-12: foreign declarations are checked by path and symbol."""

    def check_sources(self, sources, allowlist):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / "scripts").mkdir()
            for name in ("check-ffi.sh", "check_ffi.py"):
                source = ROOT / "scripts" / name
                if source.exists():
                    (directory / "scripts" / name).write_text(source.read_text())
            (directory / "scripts/ffi-allowlist.txt").write_text(allowlist)
            (directory / "crates/x/src").mkdir(parents=True)
            for name, text in sources.items():
                (directory / "crates/x/src" / name).write_text(text)
            return subprocess.run(["bash", str(directory / "scripts/check-ffi.sh")],
                                  capture_output=True, text=True)

    def test_extern_blocks_without_an_abi_with_another_abi_or_a_split_brace_are_found(self):
        result = self.check_sources({
            "listed.rs": 'extern "C" {\n    fn f();\n}\n',
            "plain.rs": 'unsafe extern {\n    fn f();\n}\n',
            "system.rs": 'extern "system" {\n    fn f();\n}\n',
            "split.rs": 'unsafe extern "C"\n{\n    fn f();\n}\n',
            "callback.rs": 'pub extern "C" fn f() {}\nextern crate core;\n',
        }, "crates/x/src/listed.rs f\n")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        for name in ("plain.rs", "system.rs", "split.rs"):
            self.assertIn("crates/x/src/" + name, result.stdout)
        self.assertNotIn("callback.rs", result.stdout, "a definition is not a declaration")

    def test_a_new_symbol_in_a_listed_file_is_rejected(self):
        result = self.check_sources({
            "listed.rs": 'unsafe extern "C" { fn f(); fn added(); }\n',
        }, "crates/x/src/listed.rs f\n")
        self.assertEqual(result.returncode, 1, "an added foreign symbol bypassed the allowlist")
        self.assertIn("added", result.stdout)

    def test_stale_symbols_are_rejected_even_when_the_file_still_declares_others(self):
        result = self.check_sources({
            "listed.rs": 'extern "C" { fn f(); }\n',
        }, "crates/x/src/listed.rs f\ncrates/x/src/listed.rs removed\n")
        self.assertEqual(result.returncode, 1, "a stale foreign symbol remained allowed")
        self.assertIn("removed", result.stdout)

    def test_static_items_comments_strings_and_callback_types_are_distinguished(self):
        result = self.check_sources({
            "listed.rs": '''
                // extern "C" { fn imaginary(); }
                /* nested /* extern { fn fake(); } */ comment */
                const EXAMPLE: &str = r#"extern { fn quoted(); }"#;
                extern "C" {
                    pub(crate) static mut port: u32;
                    fn f(callback: unsafe extern "C" fn(i32)) -> i32;
                    fn r#type();
                }
            ''',
        }, "crates/x/src/listed.rs f\ncrates/x/src/listed.rs port\ncrates/x/src/listed.rs type\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_an_empty_allowlist_and_foreign_free_sources_are_valid(self):
        result = self.check_sources({"local.rs": 'pub extern "C" fn f() {}\n'}, "# no exceptions\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_a_link_name_cannot_redirect_an_allowed_declaration(self):
        result = self.check_sources({
            "listed.rs": 'extern "C" { #[link_name = "unexpected"] fn f(); }\n',
        }, "crates/x/src/listed.rs f\n")
        self.assertEqual(result.returncode, 1, "an allowed name concealed another foreign symbol")
        self.assertIn("link_name", result.stdout)

if __name__ == "__main__":
    unittest.main()
