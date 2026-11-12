"""Product, tests and standalone probes share one M4/macOS 27 build baseline."""

import json
import shlex
import subprocess
import tempfile
import unittest
from functools import cache
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
PACKAGES = ("swift/EchodeApp", "swift/EchodeBridge", "spikes/draw", "spikes/input")


def run(arguments, **options):
    result = subprocess.run(arguments, cwd=ROOT, capture_output=True, text=True, **options, check=False)
    if result.returncode:
        raise AssertionError(result.stdout + result.stderr)
    return result.stdout


@cache
def package(path):
    # Each test only reads the manifest, and none of them edits it during the run.
    return json.loads(run(["swift", "package", "--package-path", path, "dump-package"]))


def flags(target, tool):
    return [
        flag
        for setting in target.get("settings", [])
        if setting["tool"] == tool
        for flag in setting["kind"].get("unsafeFlags", {}).get("_0", [])
    ]


class BuildPlatformTests(unittest.TestCase):
    def test_unreadable_cpu_does_not_allow_native_tests(self):
        from scripts.check_build_host import main

        error = subprocess.CalledProcessError(1, "sysctl", stderr="Operation not permitted")
        with (
            patch("scripts.check_build_host.subprocess.run", side_effect=error),
            self.assertRaisesRegex(SystemExit, "Cannot verify the M4 native-test host: Operation not permitted"),
        ):
            main()

    def test_native_test_host_requires_m4_and_macos27(self):
        from scripts.check_build_host import supports_native_tests

        for cpu in (
            "Apple M4",
            "Apple M4 Pro",
            "Apple M4 Max",
            "Apple M4 Pro (Virtual)",
            "Apple M5",
            "Apple M10 Ultra",
        ):
            self.assertTrue(supports_native_tests("Darwin", "arm64", "27.0.1", cpu))
        for system, architecture, version, cpu in (
            ("Darwin", "arm64", "27.0", "Apple M3"),
            ("Darwin", "arm64", "26.0", "Apple M4"),
            ("Darwin", "x86_64", "27.0", "Apple M4"),
            ("Darwin", "arm64", "27.0", "Unknown"),
            ("Darwin", "arm64", "", "Apple M4"),
            ("Linux", "arm64", "27.0", "Apple M4"),
        ):
            self.assertFalse(supports_native_tests(system, architecture, version, cpu))

    def test_all_swift_packages_and_c_targets_use_m4_and_macos27(self):
        for path in PACKAGES:
            manifest = package(path)
            self.assertIn({"platformName": "macos", "version": "27.0", "options": []}, manifest["platforms"])
            for target in manifest["targets"]:
                with self.subTest(package=path, target=target["name"]):
                    if target["name"] in ("CEchodeFfi", "CEchodeDraw"):
                        self.assertIn("-mcpu=apple-m4", flags(target, "c"))
                        self.assertIn("-mcpu=apple-m4", flags(target, "cxx"))
                    else:
                        swift = flags(target, "swift")
                        self.assertIn("-target-cpu", swift)
                        self.assertEqual(swift[swift.index("-target-cpu") + 1], "apple-m4")
                        self.assertIn(["-Xcc", "-mcpu=apple-m4"], [swift[i : i + 2] for i in range(len(swift))])

    def test_swift_and_clang_imports_build_in_a_fresh_cache(self):
        target = next(t for t in package("swift/EchodeBridge")["targets"] if t["name"] == "EchodeBridge")
        swift_flags = flags(target, "swift")
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for directory in ("Sources/CProbe/include", "Sources/Probe", "Tests/ProbeTests"):
                (root / directory).mkdir(parents=True)
            (root / "Package.swift").write_text(
                """// swift-tools-version: 6.0
import PackageDescription
let settings: [SwiftSetting] = [.unsafeFlags("""
                + json.dumps(swift_flags)
                + """)]
let package = Package(name: "Probe", platforms: [.macOS("27.0")], targets: [
    .target(name: "CProbe", cSettings: [.unsafeFlags(["-mcpu=apple-m4"])]),
    .target(name: "Probe", dependencies: ["CProbe"], swiftSettings: settings),
    .testTarget(name: "ProbeTests", dependencies: ["Probe"], swiftSettings: settings)
])
"""
            )
            (root / "Sources/CProbe/include/probe.h").write_text("int probe_cpu(void);\n")
            (root / "Sources/CProbe/probe.c").write_text("""#include "probe.h"
#ifndef __ARM_FEATURE_SME2
#error Expected the M4 CPU baseline
#endif
int probe_cpu(void) { return 4; }
""")
            (root / "Sources/Probe/Probe.swift").write_text(
                "import CProbe\nimport Foundation\npublic func cpuProbe() -> Int32 { probe_cpu() }\n"
            )
            (root / "Tests/ProbeTests/ProbeTests.swift").write_text(
                "import Testing\n@testable import Probe\n@Test func cpuTarget() { #expect(cpuProbe() == 4) }\n"
            )
            output = run(["swift", "test", "--package-path", str(root), "--scratch-path", str(root / "build")])
            self.assertIn("Test run with 1 test", output)
            source = root / "cpu.swift"
            source.write_text("import Foundation\npublic func cpuSum(_ a: [Float]) -> Float { a.reduce(0, +) }\n")
            ir = root / "cpu.ll"
            direct_flags = run(
                ["bash", "-c", 'source scripts/build-platform.env; printf "%s\\n" "${ECHODE_SWIFT_CPU_FLAGS[@]}"']
            ).splitlines()
            for options in (swift_flags, direct_flags):
                run(
                    [
                        "swiftc",
                        *options,
                        "-target",
                        "arm64-apple-macos27.0",
                        "-module-cache-path",
                        str(root / "modules"),
                        "-O",
                        "-emit-ir",
                        "-parse-as-library",
                        str(source),
                        "-o",
                        str(ir),
                    ]
                )
                self.assertIn('"target-cpu"="apple-m4"', ir.read_text())

    def test_c_and_cpp_dependencies_emit_m4_code(self):
        import tomllib

        configuration = tomllib.loads((ROOT / ".cargo/config.toml").read_text())
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "probe.c"
            source.write_text("int probe_cpu(int value) { return value * 4; }\n")
            for language, variable in (("c", "CFLAGS"), ("c++", "CXXFLAGS")):
                ir = root / (variable + ".ll")
                run(
                    [
                        "clang",
                        "-arch",
                        "arm64",
                        *shlex.split(configuration["env"][variable]["value"]),
                        "-x",
                        language,
                        "-O2",
                        "-S",
                        "-emit-llvm",
                        str(source),
                        "-o",
                        str(ir),
                    ]
                )
                self.assertIn('"target-cpu"="apple-m4"', ir.read_text())

    def test_direct_rust_helpers_emit_m4_code(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, ir = root / "probe.rs", root / "probe.ll"
            source.write_text('fn main() { println!("platform"); }\n')
            run(
                [
                    "bash",
                    "-c",
                    'source scripts/bridge-toolchain.env; exec rustc "+$BRIDGE_NIGHTLY" "${ECHODE_RUST_CPU_FLAGS[@]}" "$@"',
                    "platform-probe",
                    "--edition=2024",
                    "--emit=llvm-ir",
                    str(source),
                    "-o",
                    str(ir),
                ]
            )
            text = ir.read_text()
            self.assertIn('"target-cpu"="apple-m4"', text)
            self.assertRegex(text, r'target triple = "arm64-apple-macosx27\.0\.0"')
