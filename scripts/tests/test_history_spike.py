"""Keep the Phase 0 history model out of default product builds."""

import json
import subprocess
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HISTORY = ROOT / "crates/echode-history"


class HistorySpikeTests(unittest.TestCase):
    def test_spike_is_opt_in_and_its_test_requires_the_feature(self):
        manifest = tomllib.loads((HISTORY / "Cargo.toml").read_text())
        features = manifest.get("features", {})
        self.assertIn("spike", features, "history spike must be opt-in")
        self.assertEqual(features["spike"], [])
        self.assertNotIn("spike", features.get("default", []))
        tests = {test["name"]: test for test in manifest.get("test", [])}
        self.assertEqual(tests["roundtrip"]["required-features"], ["spike"])

    def test_every_benchmark_module_is_feature_gated(self):
        source = (HISTORY / "src/lib.rs").read_text()
        for module in ("format", "project", "store", "spike"):
            with self.subTest(module=module):
                self.assertRegex(
                    source,
                    rf'#\[cfg\(feature = "spike"\)\]\s+(?:pub )?mod {module};',
                )

    def test_default_workspace_does_not_enable_the_spike(self):
        result = subprocess.run(
            ["cargo", "metadata", "--locked", "--offline", "--format-version", "1"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        metadata = json.loads(result.stdout)
        history = next(package for package in metadata["packages"] if package["name"] == "echode-history")
        node = next(node for node in metadata["resolve"]["nodes"] if node["id"] == history["id"])
        self.assertNotIn("spike", node["features"])

    def test_default_and_spike_checks_remain_separate(self):
        makefile = (ROOT / "Makefile").read_text()
        base = makefile.split("check-base:\n", 1)[1].split("\n\n", 1)[0]
        self.assertIn("cargo test -q\n", base)
        self.assertIn("cargo clippy -q --all-targets -- -D warnings\n", base)
        self.assertIn("cargo clippy -q -p echode-history --features spike --all-targets -- -D warnings\n", base)
        self.assertIn("cargo test -q -p echode-history --features spike --lib --test roundtrip\n", base)
        measurement = makefile.split("spike-history:\n", 1)[1].split("\n\n", 1)[0]
        self.assertIn("-p echode-history --features spike --test roundtrip", measurement)
        self.assertIn("--include-ignored --test-threads=1", measurement)
