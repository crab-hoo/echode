"""The parallel module runner fails on any failed, erroring or empty module (ADR 0271)."""

import io
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import run_unit_tests as runner

PASSING = "import unittest\nclass T(unittest.TestCase):\n    def test_a(self):\n        pass\n"
SKIPPED = "import unittest\nclass T(unittest.TestCase):\n    @unittest.skip('x')\n    def test_b(self):\n        pass\n    def test_c(self):\n        pass\n"


class RunnerTests(unittest.TestCase):
    def run_directory(self, files):
        with tempfile.TemporaryDirectory() as directory:
            for name, text in files.items():
                (Path(directory) / name).write_text(text)
            out, err = io.StringIO(), io.StringIO()
            with redirect_stdout(out), redirect_stderr(err):
                code = runner.main(Path(directory))
            return code, out.getvalue(), err.getvalue()

    def test_passing_modules_report_their_tests_and_skips(self):
        code, out, _ = self.run_directory({"test_one.py": PASSING, "test_two.py": SKIPPED})
        self.assertEqual(code, 0)
        self.assertIn("3 tests in 2 modules passed (skipped=1)", out)

    def test_a_failure_an_import_error_or_an_empty_module_fails_and_shows_its_output(self):
        for text in (PASSING.replace("pass", "self.fail('boom')"), "import missing_module\n", "import unittest\n"):
            with self.subTest(text=text):
                code, _, err = self.run_directory({"test_one.py": PASSING, "test_bad.py": text})
                self.assertEqual(code, 1)
                self.assertIn("== test_bad", err)
                self.assertIn("1 of 2 modules failed: test_bad", err)

    def test_no_modules_is_a_failure(self):
        code, _, err = self.run_directory({})
        self.assertEqual(code, 1)
        self.assertIn("no test modules", err)

    def test_the_makefile_uses_the_runner_for_the_portable_and_local_suites(self):
        makefile = (Path(__file__).resolve().parents[2] / "Makefile").read_text()
        self.assertNotIn("unittest discover", makefile)
        self.assertEqual(makefile.count("python3 -B scripts/run_unit_tests.py"), 2)


if __name__ == "__main__":
    unittest.main()
