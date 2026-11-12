"""Task-tier cargo calls share one build of Cargo's default members (ADR 0328).

Cargo resolves features over the packages a command selects, so each `-p` set builds
its own copy of the workspace crates. These tests keep the task tier, the Swift tests
and the shipped app on one selection, and keep that selection's tests faithful to what
the app ships.
"""

import importlib.util
import re
import shlex
import subprocess
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("echode_verify", ROOT / "scripts/verify.py")
verify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)

CARGO = re.compile(r"\bcargo\s+(?:\+\S+\s+)?(build|test|clippy|run|rustc)\b[^\n]*")
SCRIPT = re.compile(r"\bscripts/[A-Za-z0-9_.-]+\.sh\b")
# The shipped app and release build what the Swift tests link.
SHIPPED = ("scripts/build-app.sh", "scripts/build-dist.sh")
# Features the tests add to the shipped build through dev-dependencies. Each only adds
# code the product never calls; anything else that the tests turn on changes what the
# contracts prove, and must be fixed or explained here (ADR 0328).
TEST_ONLY_FEATURES = {
    # HeadlessOutput for echode-document and echode-ffi tests; also on under cfg(test).
    "echode-audio-io": {"headless-output"},
    # proptest's random source.
    "rand": {"alloc", "os_rng", "std"},
    "rand_core": {"os_rng", "std"},
    # proptest's and insta's own regexes.
    "regex-syntax": {
        "default",
        "unicode",
        "unicode-age",
        "unicode-bool",
        "unicode-case",
        "unicode-gencat",
        "unicode-perl",
        "unicode-script",
        "unicode-segment",
    },
}


def task_recipes(policy):
    """Each automatic task-tier check's Makefile text and the shell scripts it runs."""
    parsed = verify.makefile_rules((ROOT / "Makefile").read_text())
    found = {}
    for check in policy["checks"]:
        if check.get("mode") != "automatic" or check.get("tier") == "full":
            continue
        rules = verify.makefile_identity(parsed, check.get("make_target", check["id"]))["rules"]
        texts = {f"Makefile {name}": "\n".join(lines[1:]) for name, lines in rules.items()}
        pending = [script for text in texts.values() for script in SCRIPT.findall(text)]
        while pending:
            script = pending.pop()
            if f"{script}" in texts:
                continue
            texts[script] = (ROOT / script).read_text()
            pending += SCRIPT.findall(texts[script])
        found[check["id"]] = texts
    return found


def joined(text):
    """Shell text with continued lines joined, so one call is one line."""
    return re.sub(r"\\\n\s*", " ", text)


def default_members():
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]
    return workspace["members"], workspace["default-members"]


def features(edges):
    """(name, version) -> features in the default members' build with these edges."""
    output = subprocess.run(
        ["cargo", "tree", "--locked", "--offline", "-e", edges, "--prefix", "none", "-f", "{p}|{f}"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    found = {}
    for line in output.splitlines():
        if "|" not in line:
            # A blank line separates the trees of the default members.
            continue
        package, _, enabled = line.removesuffix(" (*)").partition("|")
        name, version = package.split()[:2]
        found.setdefault((name, version), set()).update(item for item in enabled.split(",") if item)
    return found


class CargoSelectionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.policy = verify.load_policy(ROOT)
        members, defaults = default_members()
        names = {path: tomllib.loads((ROOT / path / "Cargo.toml").read_text())["package"]["name"] for path in members}
        cls.default_names = {names[path] for path in defaults}

    def assert_shared(self, where, line):
        """A call names packages only to add features or to reach a crate outside the default members."""
        words = shlex.split(line.split(" -- ", 1)[0])
        self.assertNotIn("--workspace", words, f"{where}: {line}")
        self.assertNotIn("--all", words, f"{where}: {line}")
        packages = [words[index + 1] for index, word in enumerate(words) if word in ("-p", "--package")]
        adds_features = "--features" in words or "--all-features" in words
        if packages and not adds_features:
            self.assertFalse(
                set(packages) & self.default_names,
                f"{where}: `-p` without features builds another copy of the default members: {line}",
            )

    def test_task_checks_build_the_default_members_once(self):
        calls = 0
        for check, texts in task_recipes(self.policy).items():
            for where, text in texts.items():
                for match in CARGO.finditer(joined(text)):
                    line = match[0]
                    # Another profile, target directory or toolchain is not target/debug's build.
                    if re.search(r"--release\b|--target-dir\b|CARGO_TARGET_DIR=|cargo\s+\+", line):
                        continue
                    calls += 1
                    with self.subTest(check=check, where=where, line=line):
                        self.assert_shared(where, line)
        self.assertGreater(calls, 10, "the scan found the task tier's cargo calls")

    def test_the_app_ships_the_build_the_swift_tests_link(self):
        for script in SHIPPED:
            builds = [match[0] for match in CARGO.finditer(joined((ROOT / script).read_text()))]
            with self.subTest(script=script):
                self.assertTrue(builds)
                for line in builds:
                    self.assertNotRegex(line, r"\s(-p|--package|--workspace|--features)\b", line)
        for target in ("check-swift", "contract-prerequisites", "check-real-hal"):
            recipe = (ROOT / "Makefile").read_text().split(f"\n{target}:\n", 1)[1].split("\n\n", 1)[0]
            with self.subTest(target=target):
                self.assertIn("\tcargo build -q --locked --offline\n", recipe + "\n")
        shell = (ROOT / "scripts/check-shell.sh").read_text()
        self.assertIn("\ncargo build -q --locked --offline\n", shell)

    def test_only_spikes_stay_out_of_the_default_members(self):
        members, defaults = default_members()
        self.assertEqual(len(set(defaults)), len(defaults))
        self.assertLessEqual(set(defaults), set(members))
        self.assertEqual({path for path in members if path not in defaults}, {"spikes/bridge"})
        self.assertEqual({path for path in members if path.startswith("crates/")}, set(defaults))

    def test_checks_on_the_shared_build_declare_every_member_manifest(self):
        # Any default member's dependencies can change the features the shared build unifies.
        manifests = "crates/*/Cargo.toml"
        checks = {check["id"]: check for check in self.policy["checks"]}
        for name in ("check-swift", "check-real-hal"):
            with self.subTest(check=name):
                self.assertIn(manifests, checks[name]["inputs"])
        self.assertIn("crates/**", checks["check"]["inputs"])
        self.assertIn(manifests, self.policy["product_contract_inputs"]["common"]["inputs"])
        for path in (ROOT / "crates").glob("*/Cargo.toml"):
            self.assertTrue(verify.matches([path.relative_to(ROOT).as_posix()], [manifests]))

    def test_tests_build_the_shipped_features_plus_declared_test_only_ones(self):
        # The contracts run from the tests' build; it must stand for the app (ADR 0328).
        shipped, tested = features("normal,build"), features("normal,build,dev")
        self.assertIn("echode-ffi", {name for name, _version in shipped})
        for (name, version), enabled in sorted(shipped.items()):
            with self.subTest(package=f"{name} {version}"):
                self.assertLessEqual(enabled, tested[(name, version)])
                self.assertLessEqual(tested[(name, version)] - enabled, TEST_ONLY_FEATURES.get(name, set()))
        # The bridge spike's uniffi turns this on; the product keeps serde_json's recursion limit.
        for (name, _version), enabled in tested.items():
            if name == "serde_json":
                self.assertNotIn("unbounded_depth", enabled)


if __name__ == "__main__":
    unittest.main()
