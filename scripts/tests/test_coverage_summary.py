"""ADR 0290: the coverage summary counts product sources by crate and Swift module."""

import json
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

import coverage_summary


def export(entries):
    return {
        "data": [
            {
                "files": [
                    {"filename": str(ROOT / name), "summary": {"lines": {"count": count, "covered": covered}}}
                    for name, count, covered in entries
                ]
            }
        ]
    }


class CoverageSummary(unittest.TestCase):
    def test_product_files_belong_to_their_crate_or_module(self):
        cases = {
            "crates/echode-model/src/model.rs": "echode-model",
            "crates/echode-sandbox/src/region/inner.rs": "echode-sandbox",
            "swift/EchodeApp/Sources/EchodeAppCore/ProjectController.swift": "swift/EchodeAppCore",
            "swift/EchodeBridge/Sources/EchodeBridge/Document.swift": "swift/EchodeBridge",
        }
        for path, unit in cases.items():
            with self.subTest(path=path):
                self.assertEqual(coverage_summary.unit(str(ROOT / path)), unit)

    def test_tests_spikes_and_outside_files_are_left_out(self):
        for path in (
            "crates/echode-model/tests/properties.rs",
            "crates/echode-sandbox/src/region_tests.rs",
            "crates/echode-fuzz/fuzz_targets/control.rs",
            "crates/echode-fuzz/src/lib.rs",
            "crates/echode-test-plugin/src/lib.rs",
            "crates/echode-actions/examples/swift_actions.rs",
            "swift/EchodeApp/Tests/EchodeAppTests/MixerTests.swift",
            "spikes/bridge/src/lib.rs",
            "target/debug/build/out.rs",
        ):
            with self.subTest(path=path):
                self.assertIsNone(coverage_summary.unit(str(ROOT / path)))
        self.assertIsNone(coverage_summary.unit("/rustc/library/core/src/lib.rs"))

    def test_lines_add_up_per_unit_and_per_language(self):
        with tempfile.TemporaryDirectory() as directory:
            rust = Path(directory) / "rust.json"
            swift = Path(directory) / "swift.json"
            rust.write_text(
                json.dumps(
                    export(
                        [
                            ("crates/echode-time/src/lib.rs", 10, 7),
                            ("crates/echode-time/src/index.rs", 30, 3),
                            ("crates/echode-time/tests/index.rs", 50, 50),
                        ]
                    )
                )
            )
            swift.write_text(json.dumps(export([("swift/EchodeApp/Sources/EchodeAppCore/A.swift", 4, 1)])))
            units = coverage_summary.summarize([rust, swift])
            self.assertEqual(
                units,
                {
                    "echode-time": {"count": 40, "covered": 10, "files": 2},
                    "swift/EchodeAppCore": {"count": 4, "covered": 1, "files": 1},
                },
            )
            table = coverage_summary.render(units)
            self.assertIn("| echode-time | 2 | 40 | 10 | 25.0% |", table)
            self.assertIn("| **Rust 計** | 2 | 40 | 10 | 25.0% |", table)
            self.assertIn("| **Swift 計** | 1 | 4 | 1 | 25.0% |", table)

    def test_no_product_source_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            empty = Path(directory) / "empty.json"
            empty.write_text(json.dumps(export([("crates/echode-time/tests/index.rs", 5, 5)])))
            self.assertEqual(coverage_summary.main([directory, str(empty)]), 1)


if __name__ == "__main__":
    unittest.main()
