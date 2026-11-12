"""Keep the Phase 0 engine workload out of default product builds."""

import json
import subprocess
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ENGINE = ROOT / "crates/echode-engine"


class EngineSpikeTests(unittest.TestCase):
    def test_spike_is_opt_in_and_its_binary_requires_the_feature(self):
        manifest = tomllib.loads((ENGINE / "Cargo.toml").read_text())
        features = manifest.get("features", {})
        self.assertIn("spike", features, "engine spike must be opt-in")
        self.assertEqual(features["spike"], [])
        self.assertNotIn("spike", features.get("default", []))
        binaries = {binary["name"]: binary for binary in manifest.get("bin", [])}
        self.assertEqual(binaries["echode-engine-spike"]["required-features"], ["spike"])

    def test_every_benchmark_module_is_feature_gated(self):
        source = (ENGINE / "src/lib.rs").read_text()
        for module in ("graph", "wait", "spike"):
            with self.subTest(module=module):
                self.assertRegex(
                    source,
                    rf'#\[cfg\(feature = "spike"\)\]\s+(?:pub )?mod {module};',
                )
        device = (ENGINE / "src/device.rs").read_text()
        self.assertRegex(
            device,
            r'#\[cfg\(feature = "spike"\)\]\s+'
            r'#\[path = "device_measurement.rs"\]\s+pub\(crate\) mod measurement;',
        )
        self.assertRegex(
            device,
            r'#\[cfg\(feature = "spike"\)\]\s+use objc2_core_audio_types::'
            r"\{AudioBuffer, AudioBufferList, AudioTimeStamp\};",
        )
        self.assertRegex(device, r'#\[cfg\(feature = "spike"\)\]\s+fn set_u32\(')
        for function in (
            "AudioObjectSetPropertyData",
            "AudioDeviceCreateIOProcID",
            "AudioDeviceDestroyIOProcID",
            "AudioDeviceStart",
            "AudioDeviceStop",
        ):
            with self.subTest(function=function):
                self.assertRegex(
                    device,
                    rf'#\[cfg\(feature = "spike"\)\]\s+use objc2_core_audio::\{{[^}}]*\b{function}\b',
                )
                self.assertNotRegex(device, rf"fn {function}\(")
        self.assertRegex(device, r'#\[cfg\(feature = "spike"\)\]\s+use libc::mach_absolute_time;')

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
        engine = next(package for package in metadata["packages"] if package["name"] == "echode-engine")
        node = next(node for node in metadata["resolve"]["nodes"] if node["id"] == engine["id"])
        self.assertNotIn("spike", node["features"])

    def test_default_and_spike_checks_remain_separate(self):
        makefile = (ROOT / "Makefile").read_text()
        base = makefile.split("check-base:\n", 1)[1].split("\n\n", 1)[0]
        self.assertIn("cargo test -q --workspace\n", base)
        self.assertIn("cargo clippy -q --workspace --all-targets -- -D warnings\n", base)
        self.assertIn("cargo build -q -p echode-engine --features spike --bin echode-engine-spike\n", base)
        self.assertIn("cargo clippy -q -p echode-engine --features spike --all-targets -- -D warnings\n", base)
        self.assertIn("cargo test -q -p echode-engine --features spike --lib\n", base)
        for target in ("spike-engine", "spike-engine-smoke"):
            with self.subTest(target=target):
                measurement = makefile.split(f"{target}:\n", 1)[1].split("\n\n", 1)[0]
                for command in measurement.splitlines():
                    if "cargo " in command:
                        self.assertIn("-p echode-engine --features spike", command)
                self.assertIn("--bin echode-engine-spike", measurement)

    def test_measurement_contract_enables_the_feature(self):
        policy = json.loads((ROOT / "scripts/verification.json").read_text())
        contract = next(
            contract for contract in policy["product_contracts"] if contract["id"] == "engine-measurement-completion"
        )
        self.assertEqual(contract["target"], ["--lib", "--features", "spike"])
        self.assertEqual(
            contract["test"], "device::measurement::tests::contract_engine_measurement_requires_progress_and_completion"
        )
        self.assertEqual(contract["negative_control"]["file"], "crates/echode-engine/src/device_measurement.rs")
