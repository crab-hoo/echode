"""Validation must use the pinned runtime and reject stale or mixed runtimes."""

import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("tsan_runtime", ROOT / "scripts/tsan_runtime.py")
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


class RuntimeTests(unittest.TestCase):
    def test_address_check_still_launches_swift_with_macos_bash(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            scripts = root / "scripts"
            scripts.mkdir()
            for name in ("check-product-bridge.sh", "run-bridge-validation.py",
                         "bridge-toolchain.env", "build-platform.env"):
                shutil.copy2(ROOT / "scripts" / name, scripts / name)
            header = root / "target/debug/build/echode-ffi-fixture/out/echode.h"
            header.parent.mkdir(parents=True)
            header.write_text("/* fixture header */")
            (root / "swift/EchodeBridge/CEchodeFfi/include").mkdir(parents=True)
            tools = root / "tools"
            tools.mkdir()
            (tools / "cargo").write_text("#!/bin/sh\nexit 0\n")
            (tools / "swift").write_text(
                '#!/bin/sh\nprintf "%s\\n" __SWIFT_CALL__ "$@" >> "$ECHODE_TEST_COMMAND_LOG"\n'
                'case "$*" in *"--filter RecoveryConsentTests"*) '
                'if [ "${ECHODE_TEST_FAIL_RECOVERY:-}" = 1 ]; then exit 7; fi;; esac\n'
                'echo "Test run with ${ECHODE_TEST_COUNT:-2} tests in 1 suite passed after 0.001 seconds."\n')
            for tool in tools.iterdir():
                tool.chmod(0o755)
            log = root / "swift-arguments.log"
            result = subprocess.run(["/bin/bash", str(scripts / "check-product-bridge.sh"), "address"],
                                    capture_output=True, text=True, timeout=10, env={
                                        **os.environ, "PATH": str(tools) + ":" + os.environ["PATH"],
                                        "ECHODE_TEST_COMMAND_LOG": str(log),
                                    })
            self.assertEqual(result.returncode, 0, result.stderr)
            calls = [call.splitlines() for call in log.read_text().split("__SWIFT_CALL__\n")[1:]]
            self.assertEqual(len(calls), 2)
            for arguments in calls:
                self.assertEqual(arguments[0], "test")
                self.assertEqual(arguments[arguments.index("--sanitize") + 1], "address")
            self.assertEqual(calls[0][calls[0].index("--skip") + 1], "RecoveryConsentTests")
            self.assertEqual(calls[1][calls[1].index("--filter") + 1], "RecoveryConsentTests")
            self.assertIn("--skip-build", calls[1])
            failed = subprocess.run(["/bin/bash", str(scripts / "check-product-bridge.sh"), "address"],
                                    capture_output=True, text=True, timeout=10, env={
                                        **os.environ, "PATH": str(tools) + ":" + os.environ["PATH"],
                                        "ECHODE_TEST_COMMAND_LOG": str(log), "ECHODE_TEST_FAIL_RECOVERY": "1",
                                    })
            self.assertNotEqual(failed.returncode, 0)
            self.assertIn("recovery-swift-tests.failed.log", failed.stderr)
            empty = subprocess.run(["/bin/bash", str(scripts / "check-product-bridge.sh"), "address"],
                                   capture_output=True, text=True, timeout=10, env={
                                       **os.environ, "PATH": str(tools) + ":" + os.environ["PATH"],
                                       "ECHODE_TEST_COMMAND_LOG": str(log), "ECHODE_TEST_COUNT": "0",
                                   })
            self.assertNotEqual(empty.returncode, 0)
            self.assertIn("recovery suite must execute at least one test", empty.stderr)

    def test_both_runtime_artifacts_and_the_build_context_must_match(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(runtime, "CACHE", Path(directory)):
            libraries = {}
            for variant in ("baseline", "fixed"):
                path = runtime.CACHE / variant / runtime.LIBRARY
                path.parent.mkdir()
                path.write_bytes(variant.encode())
                libraries[variant] = runtime.sha256(path)
            manifest = runtime.CACHE / "manifest.json"
            manifest.write_text(json.dumps({"context": {"fixture": 1}, "libraries": libraries}))
            self.assertTrue(runtime.prepared({"fixture": 1}))
            self.assertFalse(runtime.prepared({"fixture": 2}))
            (runtime.CACHE / "baseline" / runtime.LIBRARY).write_bytes(b"changed")
            self.assertFalse(runtime.prepared({"fixture": 1}))
            manifest.write_text("{invalid")
            self.assertFalse(runtime.prepared({"fixture": 1}))

    def test_validation_refuses_missing_or_mixed_loader_provenance(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            library = root / runtime.LIBRARY
            library.write_bytes(b"loader protocol fixture, never loaded")
            for libraries, accepted in (([], False), ([library], True),
                                         ([library, root / "other" / runtime.LIBRARY], False)):
                lines = "\n".join(f"dyld[42]: <fixture> {path}" for path in libraries)
                result = subprocess.run([
                    sys.executable, str(ROOT / "scripts/run-bridge-validation.py"),
                    "--log", str(root / "probe.log"), "--tsan-runtime", str(library),
                    sys.executable, "-c", f"print({lines!r})",
                ], capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode == 0, accepted, result.stderr)

    def test_a_failed_run_keeps_its_log_and_shows_the_report_not_loader_lines(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            log = root / "swift-tests.log"
            report = ("WARNING: ThreadSanitizer: data race (pid=1)\n  Read of size 8 by thread T8:\n"
                      "SUMMARY: ThreadSanitizer: data race Document.swift:59 in fixture\n")
            noise = "dyld[1]: <fixture> /usr/lib/fixture.dylib\n" * 2000
            program = f"import sys; sys.stderr.write({(noise + report + noise)!r}); sys.exit(6)"
            result = subprocess.run([sys.executable, str(ROOT / "scripts/run-bridge-validation.py"),
                                     "--log", str(log), sys.executable, "-c", program],
                                    capture_output=True, text=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("SUMMARY: ThreadSanitizer: data race Document.swift:59", result.stderr)
            self.assertNotIn("fixture.dylib", result.stderr)
            kept = root / "swift-tests.failed.log"
            self.assertIn(str(kept), result.stderr)
            # A passing rerun rewrites the log but leaves the failing evidence.
            subprocess.run([sys.executable, str(ROOT / "scripts/run-bridge-validation.py"),
                            "--log", str(log), sys.executable, "-c", "print('ok')"], check=True, timeout=10)
            self.assertEqual(log.read_text(), "ok\n")
            self.assertIn(report, kept.read_text())


if __name__ == "__main__":
    unittest.main()
