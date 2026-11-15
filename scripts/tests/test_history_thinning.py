"""Verify opt-in feature boundary for history thinning (DIA-11)."""

from __future__ import annotations

import json
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class HistoryThinningFeatureTest(unittest.TestCase):
    def test_thinning_is_opt_in_and_checked_in_spikes_and_portable(self) -> None:
        metadata = json.loads(
            subprocess.check_output(
                ["cargo", "metadata", "--no-deps", "--format-version", "1"],
                cwd=ROOT,
                text=True,
            )
        )
        history = next(package for package in metadata["packages"] if package["name"] == "echode-history")
        self.assertEqual(history["features"].get("default", []), [])
        self.assertEqual(
            history["features"].get("thinning"),
            ["dep:flate2", "dep:gix-pack"],
        )

        deps = {dep["name"]: dep for dep in history["dependencies"]}
        self.assertTrue(deps["flate2"]["optional"])
        self.assertTrue(deps["gix-pack"]["optional"])

        targets = {target["name"]: target for target in history["targets"]}
        for name in ("thin", "thin_errors"):
            self.assertEqual(targets[name].get("required-features"), ["thinning"])

        makefile = (ROOT / "Makefile").read_text()
        self.assertIn(
            "cargo clippy -q -p echode-history --features thinning --all-targets -- -D warnings\n",
            makefile,
        )
        self.assertIn(
            "cargo test -q -p echode-history --features thinning --lib --test thin --test thin_errors\n",
            makefile,
        )
        self.assertIn(
            "cargo clippy -q --locked -p echode-history --features thinning --all-targets $(PORTABLE_BUILD) -- -D warnings\n",
            makefile,
        )


if __name__ == "__main__":
    unittest.main()
