"""The distribution bundle is signed with one self-signed identity, and the helper is its own app."""

import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
CLANG = ["clang", "-arch", "arm64", "-mcpu=apple-m4", "-mmacosx-version-min=27.0"]


def module():
    spec = importlib.util.spec_from_file_location("distribution", ROOT / "scripts" / "distribution.py")
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


distribution = module()


def project_spec():
    script = 'require "yaml"; require "json"; puts JSON.generate(YAML.load_file(ARGV[0]))'
    completed = subprocess.run(
        ["ruby", "-ryaml", "-rjson", "-e", script, str(ROOT / "project.yml")],
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(completed.stdout)


class DistributionSpec(unittest.TestCase):
    def test_cargo_emits_m4_and_macos27_despite_an_old_environment_target(self):
        import os
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text('[package]\nname="platform-contract"\nversion="0.0.0"\nedition="2024"\n')
            (root / "src/main.rs").write_text('fn main() { println!("platform"); }\n')
            env = dict(os.environ, MACOSX_DEPLOYMENT_TARGET="11.0", CARGO_TARGET_DIR=str(root / "target"))
            subprocess.run(["cargo", "rustc", "--offline", "--manifest-path", str(root / "Cargo.toml"), "--", "--emit=llvm-ir"],
                           cwd=ROOT, env=env, capture_output=True, text=True, check=True)
            ir = '\n'.join(path.read_text() for path in (root / "target").rglob('*.ll'))
            self.assertIn('"target-cpu"="apple-m4"', ir)
            self.assertRegex(ir, r'target triple = "arm64-apple-macosx27\.0\.0"')
            distribution.validate_binary(root / "target/debug/platform-contract")

    def test_release_dylib_can_be_loaded_by_macos27(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text('[package]\nname="platform-dylib"\nversion="0.0.0"\nedition="2024"\n[lib]\ncrate-type=["cdylib"]\n')
            (root / "src/lib.rs").write_text('#[unsafe(no_mangle)] pub extern "C" fn answer() -> usize { std::env::args_os().count() + 1 }\n')
            subprocess.run(["cargo", "build", "--offline", "--release", "--manifest-path", str(root / "Cargo.toml"),
                            "--target-dir", str(root / "target")], cwd=ROOT, capture_output=True, text=True, check=True)
            binary = root / "target/release/libplatform_dylib.dylib"
            distribution.validate_binary(binary)
            probe = 'import ctypes, sys; library = ctypes.CDLL(sys.argv[1]); library.answer.restype = ctypes.c_size_t; assert library.answer() > 0'
            loaded = subprocess.run([sys.executable, "-c", probe, str(binary)], capture_output=True, text=True)
            self.assertEqual(loaded.returncode, 0, loaded.stderr)

    def test_old_deployment_target_is_rejected_before_bundle_assembly(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "main.c"
            source.write_text("int main(void) { return 0; }\n")
            binary = root / "old"
            subprocess.run(CLANG + ["-mmacosx-version-min=11.0", str(source), "-o", str(binary)], check=True)
            app = root / "Echode.app"
            with self.assertRaisesRegex(RuntimeError, "minimum macOS"):
                distribution.assemble_app(app, binary, binary, ROOT / "swift/EchodeApp/Info.plist")
            self.assertFalse(app.exists())

    def test_the_xcode_project_uses_the_shared_certificate(self):
        spec = project_spec()
        settings = spec["settings"]["base"]
        self.assertEqual(settings["CODE_SIGN_IDENTITY"], "Echode Local")
        self.assertEqual(settings["CODE_SIGN_STYLE"], "Manual")
        self.assertIs(settings["ENABLE_APP_SANDBOX"], False)
        self.assertEqual(settings["MACOSX_DEPLOYMENT_TARGET"], "27.0")
        self.assertEqual(settings["ARCHS"], "arm64")
        self.assertIn("-mcpu=apple-m4", settings["OTHER_CFLAGS"])
        self.assertIn("-mcpu=apple-m4", settings["OTHER_CPLUSPLUSFLAGS"])
        self.assertEqual(settings["OTHER_SWIFT_FLAGS"][-4:], ["-target-cpu", "apple-m4", "-Xcc", "-mcpu=apple-m4"])
        target = spec["targets"]["Echode"]
        self.assertEqual(target["settings"]["base"]["PRODUCT_BUNDLE_IDENTIFIER"], "io.echode.app")
        self.assertEqual(target["entitlements"]["path"], "Packaging/Echode.entitlements")
        self.assertEqual(target["settings"]["base"]["INFOPLIST_FILE"], "swift/EchodeApp/Info.plist")
        plist = (ROOT / "swift/EchodeApp/Info.plist").read_text(encoding="utf-8")
        self.assertIn("io.echode.app", plist)
        self.assertNotIn("$(PRODUCT_BUNDLE_IDENTIFIER)", plist)
        self.assertEqual(spec["packages"]["EchodeApp"]["path"], "swift/EchodeApp")
        self.assertIn("Echode Distribution", spec["schemes"])
        script = target["postBuildScripts"][0]["script"]
        self.assertIn("scripts/build-dist.sh", script)
        source_paths = [item["path"] if isinstance(item, dict) else item for item in target["sources"]]
        self.assertIn("Packaging/EchodePluginHost", source_paths)
        text = (ROOT / "project.yml").read_text(encoding="utf-8")
        self.assertNotIn("disable-library-validation", text)
        self.assertNotIn('CODE_SIGN_IDENTITY: "-"', text)
        self.assertEqual(settings["ENABLE_HARDENED_RUNTIME"], True)

    def test_library_validation_is_disabled_only_on_the_helper(self):
        helper = (ROOT / "Packaging/EchodePluginHost/EchodePluginHost.entitlements").read_text(encoding="utf-8")
        app = (ROOT / "Packaging/Echode.entitlements").read_text(encoding="utf-8")
        info = (ROOT / "Packaging/EchodePluginHost/Info.plist").read_text(encoding="utf-8")
        self.assertIn("com.apple.security.cs.disable-library-validation", helper)
        self.assertNotIn("disable-library-validation", app)
        self.assertNotIn("app-sandbox", app)
        self.assertIn("io.echode.plugin-host", info)
        self.assertIn("<key>LSUIElement</key><true/>", info.replace(" ", "").replace("\n", ""))


class SignedBundle(unittest.TestCase):
    def test_a_temporary_identity_signs_the_helper_app_and_leaves_the_main_app_strict(self):
        password = "echode-test"
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            keychain = root / "Echode.keychain-db"
            try:
                distribution.create_signing_keychain(keychain, password)
                app = self.signed_app(root, keychain)
                self.assertTrue((app / "Contents/Helpers/EchodePluginHost.app/Contents/MacOS/echode-plugin-host").is_file())
                cli = app / "Contents/MacOS/echode-cli"
                self.assertTrue(cli.is_file())
                subprocess.run(["codesign", "--verify", "--strict", str(cli)], check=True)
                self.assertNotIn("disable-library-validation", self.entitlements(cli))
                self.assertTrue((app / "Contents/Frameworks/EchodeFFI.xcframework").is_dir())
                self.assertFalse((app / "Contents/MacOS/echode-plugin-host").exists())
                subprocess.run(["codesign", "--verify", "--strict", str(app)], check=True)
                plugin = app / "Contents/PlugIns/EchodeSinePad.clap"
                self.assertTrue(distribution.plugin_binary(plugin).is_file())
                subprocess.run(["codesign", "--verify", "--strict", str(plugin)], check=True)
                self.assertIn("Authority=Echode Local", self.codesign_details(plugin))
                described = self.codesign_details(app)
                self.assertIn("Authority=Echode Local", described)
                self.assertNotIn("Signature=adhoc", described)
                self.assertIn("Identifier=io.echode.app", described)
                helper = self.codesign_details(app / "Contents/Helpers/EchodePluginHost.app")
                self.assertIn("Authority=Echode Local", helper)
                self.assertIn("disable-library-validation", self.entitlements(app / "Contents/Helpers/EchodePluginHost.app"))
                self.assertNotIn("disable-library-validation", self.entitlements(app))
                self.assertIn("(runtime)", described)
                self.assertIn("(runtime)", helper)
                foreign = self.foreign_library(root)
                main = app / "Contents/MacOS/Echode"
                helper_bin = app / "Contents/Helpers/EchodePluginHost.app/Contents/MacOS/echode-plugin-host"
                accepted = subprocess.run([str(helper_bin), str(foreign)], capture_output=True, text=True)
                self.assertEqual(accepted.returncode, 0, accepted.stderr)
                with self.subTest("the main app refuses the library at load time"):
                    if not self.host_enforces_library_validation(root, foreign):
                        # ADR 0269: a hosted runner VM may not enforce it; a developer's Mac must.
                        if os.environ.get("GITHUB_ACTIONS") != "true":
                            self.fail("this Mac does not enforce library validation, so the main app cannot be checked")
                        self.skipTest("the runner does not enforce library validation")
                    rejected = subprocess.run([str(main), str(foreign)], capture_output=True, text=True)
                    self.assertEqual(rejected.returncode, 1, rejected.stderr)
            finally:
                subprocess.run(["security", "delete-keychain", str(keychain)], check=False)

    def signed_app(self, root, keychain):
        source = root / "probe.c"
        source.write_text("void echode_distribution_probe(void) {}\n", encoding="utf-8")
        object_file = root / "probe.o"
        library = root / "libprobe.a"
        subprocess.run(CLANG + ["-c", str(source), "-o", str(object_file)], check=True)
        subprocess.run(["libtool", "-static", "-o", str(library), str(object_file)], check=True)
        headers = root / "include"
        headers.mkdir()
        (headers / "echode.h").write_text("void echode_distribution_probe(void);\n", encoding="utf-8")
        framework = root / "EchodeFFI.xcframework"
        distribution.create_xcframework(library, headers, framework)
        executable = root / "Echode"
        helper = root / "echode-plugin-host"
        self.compile_loader(root / "loader.c", executable)
        shutil.copy(executable, helper)
        app = root / "Echode.app"
        plugin = root / "EchodeSinePad.clap"
        binary = plugin / "Contents/MacOS/EchodeSinePad"
        binary.parent.mkdir(parents=True)
        shutil.copy2(self.foreign_library(root), binary)
        shutil.copy2(ROOT / "Packaging/EchodeSinePad-Info.plist", plugin / "Contents/Info.plist")
        distribution.assemble_app(app, executable, helper, ROOT / "swift/EchodeApp/Info.plist", framework, plugins=[plugin], cli=executable)
        distribution.sign_app(app, keychain=keychain)
        return app

    def compile_loader(self, source, executable):
        source.write_text(
            "#include <dlfcn.h>\n#include <stdio.h>\nint main(int argc, char **argv) {\n"
            "    if (argc < 2) return 0;\n"
            "    if (dlopen(argv[1], RTLD_NOW) != 0) return 0;\n"
            "    fprintf(stderr, \"%s\\n\", dlerror());\n"
            "    return 1;\n"
            "}\n",
            encoding="utf-8",
        )
        subprocess.run(
            CLANG + ["-o", str(executable), str(source)],
            check=True,
        )

    def host_enforces_library_validation(self, root, library):
        # Control: a hardened, ad-hoc signed loader must refuse an ad-hoc library ("different Team IDs").
        control = root / "control"
        self.compile_loader(root / "control.c", control)
        subprocess.run(["codesign", "--force", "--sign", "-", "--options", "runtime", str(control)],
                       check=True, capture_output=True)
        completed = subprocess.run([str(control), str(library)], capture_output=True, text=True)
        self.assertIn(completed.returncode, (0, 1), completed.stderr)
        return completed.returncode == 1

    def foreign_library(self, root):
        source = root / "foreign.c"
        library = root / "libforeign.dylib"
        source.write_text("void echode_foreign(void) {}\n", encoding="utf-8")
        subprocess.run(
            CLANG + ["-dynamiclib", "-o", str(library), str(source)],
            check=True,
        )
        subprocess.run(["codesign", "--force", "--sign", "-", str(library)], check=True)
        return library

    def codesign_details(self, path):
        completed = subprocess.run(["codesign", "-dvvv", str(path)], capture_output=True, text=True)
        return completed.stdout + completed.stderr

    def entitlements(self, path):
        completed = subprocess.run(
            ["codesign", "-d", "--entitlements", "-", str(path)],
            capture_output=True,
            text=True,
        )
        return completed.stdout + completed.stderr


if __name__ == "__main__":
    unittest.main()
