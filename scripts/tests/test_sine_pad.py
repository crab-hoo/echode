"""FCA-15: the bundled instrument must not link the host's audio engine."""

import json
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class SinePadDependencies(unittest.TestCase):
    def test_the_bundled_instrument_does_not_depend_on_the_host_engine(self):
        result = subprocess.run(
            ["cargo", "metadata", "--offline", "--no-deps", "--format-version", "1"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=True,
        )
        packages = {package["name"]: package for package in json.loads(result.stdout)["packages"]}
        pending = ["echode-sine-pad"]
        reached = set()
        while pending:
            name = pending.pop()
            if name in reached:
                continue
            reached.add(name)
            package = packages.get(name)
            if package:
                pending.extend(
                    dependency["name"] for dependency in package["dependencies"] if dependency["kind"] != "dev"
                )
        self.assertNotIn("echode-engine", reached, "Sine Pad links the host engine")

    def test_dsp_edits_keep_audio_and_isolation_verification_selected(self):
        policy = json.loads((ROOT / "scripts/verification.json").read_text())
        for area in ("audio", "isolation", "render", "product_audio"):
            self.assertIn("crates/echode-dsp/**", policy["areas"][area], area)


if __name__ == "__main__":
    unittest.main()
