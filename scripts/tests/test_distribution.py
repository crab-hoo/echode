"""The distribution bundle is signed with one self-signed identity, and the helper is its own app."""

import importlib.util
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]


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
    def test_the_xcode_project_uses_the_shared_certificate(self):
        spec = project_spec()
        settings = spec["settings"]["base"]
        self.assertEqual(settings["CODE_SIGN_IDENTITY"], "Echode Local")
        self.assertEqual(settings["CODE_SIGN_STYLE"], "Manual")
        self.assertIs(settings["ENABLE_APP_SANDBOX"], False)
        self.assertEqual(settings["MACOSX_DEPLOYMENT_TARGET"], "27.0")
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
                self.assertTrue((app / "Contents/Frameworks/EchodeFFI.xcframework").is_dir())
                self.assertFalse((app / "Contents/MacOS/echode-plugin-host").exists())
                subprocess.run(["codesign", "--verify", "--strict", str(app)], check=True)
                described = self.codesign_details(app)
                self.assertIn("Authority=Echode Local", described)
                self.assertNotIn("Signature=adhoc", described)
                self.assertIn("Identifier=io.echode.app", described)
                helper = self.codesign_details(app / "Contents/Helpers/EchodePluginHost.app")
                self.assertIn("Authority=Echode Local", helper)
                self.assertIn("disable-library-validation", self.entitlements(app / "Contents/Helpers/EchodePluginHost.app"))
                self.assertNotIn("disable-library-validation", self.entitlements(app))
            finally:
                subprocess.run(["security", "delete-keychain", str(keychain)], check=False)

    def signed_app(self, root, keychain):
        source = root / "probe.c"
        source.write_text("void echode_distribution_probe(void) {}\n", encoding="utf-8")
        object_file = root / "probe.o"
        library = root / "libprobe.a"
        subprocess.run(["clang", "-arch", "arm64", "-c", str(source), "-o", str(object_file)], check=True)
        subprocess.run(["libtool", "-static", "-o", str(library), str(object_file)], check=True)
        headers = root / "include"
        headers.mkdir()
        (headers / "echode.h").write_text("void echode_distribution_probe(void);\n", encoding="utf-8")
        framework = root / "EchodeFFI.xcframework"
        distribution.create_xcframework(library, headers, framework)
        executable = root / "Echode"
        helper = root / "echode-plugin-host"
        shutil.copy("/usr/bin/true", executable)
        shutil.copy("/usr/bin/true", helper)
        app = root / "Echode.app"
        distribution.assemble_app(app, executable, helper, ROOT / "swift/EchodeApp/Info.plist", framework)
        distribution.sign_app(app, keychain=keychain)
        return app

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
