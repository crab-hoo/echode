#!/usr/bin/env python3
"""Assemble and sign the Echode distribution bundle.

The login keychain is used only by `setup-identity`. Tests pass a temporary keychain.
"""

import argparse
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
IDENTITY = "Echode Local"
HELPER_APP = "EchodePluginHost.app"
HELPER_EXECUTABLE = "echode-plugin-host"
APP_ENTITLEMENTS = ROOT / "Packaging" / "Echode.entitlements"
HELPER_ENTITLEMENTS = ROOT / "Packaging" / "EchodePluginHost" / "EchodePluginHost.entitlements"
HELPER_INFO = ROOT / "Packaging" / "EchodePluginHost" / "Info.plist"

OPENSSL_CONFIG = """
[ req ]
distinguished_name = req_distinguished_name
x509_extensions = codesign_ext
prompt = no
[ req_distinguished_name ]
CN = Echode Local
[ codesign_ext ]
basicConstraints = critical,CA:FALSE
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
"""


def login_keychain():
    return Path.home() / "Library" / "Keychains" / "login.keychain-db"


def identity_installed(keychain):
    completed = subprocess.run(
        ["security", "find-certificate", "-c", IDENTITY, str(keychain)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    return completed.returncode == 0


def run(command):
    completed = subprocess.run(command, text=True, capture_output=True, check=False)
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout).strip()
        raise RuntimeError(f"{command[0]} failed: {detail}")
    return completed


def create_signing_keychain(keychain, password):
    """Create a temporary keychain that holds the Echode Local identity."""
    if keychain.exists():
        subprocess.run(["security", "delete-keychain", str(keychain)], check=False)
    run(["security", "create-keychain", "-p", password, str(keychain)])
    run(["security", "set-keychain-settings", "-lut", "3600", str(keychain)])
    run(["security", "unlock-keychain", "-p", password, str(keychain)])
    import_identity(keychain, password, keychain_password=password)


def setup_login_identity():
    keychain = login_keychain()
    if identity_installed(keychain):
        print(f"{IDENTITY} is already in the login keychain")
        return
    import_identity(keychain, p12_password=None, keychain_password=None)
    print(f"Imported {IDENTITY} into the login keychain")
    print("The first codesign may ask for the login keychain password.")


def import_identity(keychain, p12_password, keychain_password):
    with tempfile.TemporaryDirectory() as temporary:
        work = Path(temporary)
        config = work / "codesign.cnf"
        config.write_text(OPENSSL_CONFIG, encoding="utf-8")
        key = work / "key.pem"
        cert = work / "cert.pem"
        p12 = work / "echode.p12"
        password = p12_password if p12_password is not None else "echode-local"
        run(
            [
                "openssl",
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                str(key),
                "-out",
                str(cert),
                "-days",
                "3650",
                "-config",
                str(config),
                "-extensions",
                "codesign_ext",
            ]
        )
        run(
            [
                "openssl",
                "pkcs12",
                "-export",
                "-legacy",
                "-inkey",
                str(key),
                "-in",
                str(cert),
                "-out",
                str(p12),
                "-passout",
                f"pass:{password}",
                "-name",
                IDENTITY,
            ]
        )
        run(
            [
                "security",
                "import",
                str(p12),
                "-k",
                str(keychain),
                "-P",
                password,
                "-T",
                "/usr/bin/codesign",
                "-T",
                "/usr/bin/security",
            ]
        )
    if keychain_password is not None:
        run(
            [
                "security",
                "set-key-partition-list",
                "-S",
                "apple-tool:,apple:,codesign:",
                "-s",
                "-k",
                keychain_password,
                str(keychain),
            ]
        )


def assemble_app(app, executable, helper, info_plist, xcframework=None, resources=(), plugins=(), cli=None):
    validate_binary(executable)
    validate_binary(helper)
    if cli is not None:
        validate_binary(cli)
    for plugin in plugins:
        validate_binary(plugin_binary(plugin))
    if app.exists():
        shutil.rmtree(app)
    macos = app / "Contents" / "MacOS"
    macos.mkdir(parents=True)
    _install_executable(executable, macos / "Echode")
    if cli is not None:
        _install_executable(cli, macos / "echode-cli")
    shutil.copy2(info_plist, app / "Contents" / "Info.plist")
    embed_helper(app, helper)
    if xcframework is not None:
        destination = app / "Contents" / "Frameworks" / xcframework.name
        destination.parent.mkdir(parents=True)
        shutil.copytree(xcframework, destination)
    if resources:
        resource_dir = app / "Contents" / "Resources"
        resource_dir.mkdir(parents=True)
        for bundle in resources:
            shutil.copytree(bundle, resource_dir / bundle.name)

    if plugins:
        plugin_dir = app / "Contents" / "PlugIns"
        plugin_dir.mkdir(parents=True)
        for plugin in plugins:
            shutil.copytree(plugin, plugin_dir / plugin.name)


def plugin_binary(plugin):
    with (plugin / "Contents" / "Info.plist").open("rb") as source:
        name = plistlib.load(source)["CFBundleExecutable"]
    if Path(name).name != name:
        raise RuntimeError("invalid plugin executable name")
    return plugin / "Contents" / "MacOS" / name


def embed_helper(app, helper):
    macos = app / "Contents" / "Helpers" / HELPER_APP / "Contents" / "MacOS"
    macos.mkdir(parents=True, exist_ok=True)
    _install_executable(helper, macos / HELPER_EXECUTABLE)
    shutil.copy2(HELPER_INFO, macos.parent / "Info.plist")


def _install_executable(source, destination):
    shutil.copy2(source, destination)
    destination.chmod(destination.stat().st_mode | 0o755)


def validate_binary(path):
    architectures = run(["lipo", "-archs", str(path)]).stdout.split()
    if architectures != ["arm64"]:
        raise RuntimeError(f"{path}: expected arm64 only, got {architectures}")
    builds = run(["xcrun", "vtool", "-show-build", str(path)]).stdout
    minimums = re.findall(r"^\s*minos\s+(\S+)", builds, re.MULTILINE)
    if not minimums or any(value not in ("27.0", "27.0.0") for value in minimums):
        raise RuntimeError(f"{path}: minimum macOS must be 27.0, got {minimums}")


def create_xcframework(library, headers, output):
    if output.exists():
        shutil.rmtree(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    run(
        [
            "xcodebuild",
            "-create-xcframework",
            "-library",
            str(library),
            "-headers",
            str(headers),
            "-output",
            str(output),
        ]
    )


def sign_app(app, identity=IDENTITY, keychain=None):
    helper_app = app / "Contents" / "Helpers" / HELPER_APP
    if helper_app.is_dir():
        _codesign(helper_app / "Contents" / "MacOS" / HELPER_EXECUTABLE, identity, keychain, None)
        _codesign(helper_app, identity, keychain, HELPER_ENTITLEMENTS)
    sibling = app / "Contents" / "MacOS" / HELPER_EXECUTABLE
    if sibling.is_file():
        _codesign(sibling, identity, keychain, None)
    cli = app / "Contents" / "MacOS" / "echode-cli"
    if cli.is_file():
        _codesign(cli, identity, keychain, None)
    framework = app / "Contents" / "Frameworks" / "EchodeFFI.xcframework"
    if framework.is_dir():
        _codesign(framework, identity, keychain, None)
    for plugin in sorted((app / "Contents" / "PlugIns").glob("*.clap")):
        validate_binary(plugin_binary(plugin))
        _codesign(plugin_binary(plugin), identity, keychain, None)
        _codesign(plugin, identity, keychain, None)
    _codesign(app / "Contents" / "MacOS" / "Echode", identity, keychain, None)
    _codesign(app, identity, keychain, APP_ENTITLEMENTS)


def sign_development(app):
    """Sign a local bundle with Echode Local when that certificate exists."""
    if identity_installed(login_keychain()):
        sign_app(app)
        return IDENTITY
    for plugin in sorted((app / "Contents" / "PlugIns").glob("*.clap")):
        run(["codesign", "--force", "--sign", "-", str(plugin_binary(plugin))])
        run(["codesign", "--force", "--sign", "-", str(plugin)])
    cli = app / "Contents" / "MacOS" / "echode-cli"
    if cli.is_file():
        run(["codesign", "--force", "--sign", "-", str(cli)])
    run(["codesign", "--force", "--sign", "-", str(app)])
    return "adhoc"


def _codesign(path, identity, keychain, entitlements):
    # Hardened Runtime makes library validation the default. The helper's
    # entitlement is what turns that check off for foreign plugins.
    command = ["codesign", "--force", "--sign", identity, "--timestamp=none", "--options", "runtime"]
    if keychain is not None:
        command.extend(["--keychain", str(keychain)])
    if entitlements is not None:
        command.extend(["--entitlements", str(entitlements)])
    command.append(str(path))
    run(command)


def main(argv):
    parser = argparse.ArgumentParser(description="Assemble and sign the Echode distribution bundle.")
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("identity-installed")
    sub.add_parser("setup-identity")

    created = sub.add_parser("create-keychain")
    created.add_argument("--keychain", type=Path, required=True)
    created.add_argument("--password", required=True)

    assembled = sub.add_parser("assemble")
    assembled.add_argument("--app", type=Path, required=True)
    assembled.add_argument("--executable", type=Path, required=True)
    assembled.add_argument("--helper", type=Path, required=True)
    assembled.add_argument("--cli", type=Path)
    assembled.add_argument("--info", type=Path, required=True)
    assembled.add_argument("--xcframework", type=Path)
    assembled.add_argument("--resource", type=Path, action="append", default=[])

    embedded = sub.add_parser("embed-helper")
    embedded.add_argument("--app", type=Path, required=True)
    embedded.add_argument("--helper", type=Path, required=True)

    signed = sub.add_parser("sign")
    signed.add_argument("--app", type=Path, required=True)
    signed.add_argument("--keychain", type=Path)

    development = sub.add_parser("sign-dev")
    development.add_argument("--app", type=Path, required=True)

    framework = sub.add_parser("xcframework")
    framework.add_argument("--library", type=Path, required=True)
    framework.add_argument("--headers", type=Path, required=True)
    framework.add_argument("--output", type=Path, required=True)

    assembled.add_argument("--plugin", type=Path, action="append", default=[])

    args = parser.parse_args(argv)
    if args.command == "identity-installed":
        return 0 if identity_installed(login_keychain()) else 1
    if args.command == "setup-identity":
        setup_login_identity()
    elif args.command == "create-keychain":
        create_signing_keychain(args.keychain, args.password)
    elif args.command == "assemble":
        assemble_app(
            args.app,
            args.executable,
            args.helper,
            args.info,
            args.xcframework,
            args.resource,
            args.plugin,
            args.cli,
        )
    elif args.command == "embed-helper":
        embed_helper(args.app, args.helper)
    elif args.command == "sign":
        sign_app(args.app, keychain=args.keychain)
    elif args.command == "sign-dev":
        print(sign_development(args.app))
    elif args.command == "xcframework":
        create_xcframework(args.library, args.headers, args.output)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main(sys.argv[1:]))
    except RuntimeError as error:
        print(error, file=sys.stderr)
        raise SystemExit(1) from error
