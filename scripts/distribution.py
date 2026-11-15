#!/usr/bin/env python3
"""Assemble and sign the Echode distribution bundle.

The login keychain is used only by `setup-identity`. Tests pass a temporary keychain.
"""

import argparse
import json
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


def macho_uuids(path):
    """Return sorted (UUID, arch) pairs reported by `xcrun dwarfdump -u`."""
    output = run(["xcrun", "dwarfdump", "-u", str(path)]).stdout
    uuids = sorted(
        (match.group(1).upper(), match.group(2))
        for match in re.finditer(r"^UUID:\s+([0-9A-Fa-f-]+)\s+\(([^)]+)\)", output, re.MULTILINE)
    )
    if not uuids:
        raise RuntimeError(f"{path}: no Mach-O UUID found")
    return uuids


def has_embedded_dwarf(path):
    """True when the linked Mach-O contains a `__DWARF` segment."""
    return "segname __DWARF" in run(["otool", "-l", str(path)]).stdout


def _git_revision(git_root):
    if git_root is None:
        return "unknown"
    head = subprocess.run(
        ["git", "-C", str(git_root), "rev-parse", "--short=12", "HEAD"],
        capture_output=True,
        text=True,
        check=False,
    )
    if head.returncode != 0:
        return "unknown"
    commit = head.stdout.strip() or "unknown"
    dirty = subprocess.run(
        ["git", "-C", str(git_root), "status", "--porcelain"],
        capture_output=True,
        text=True,
        check=False,
    )
    if dirty.returncode == 0 and dirty.stdout.strip():
        return f"{commit}-dirty"
    return commit


def version_tag(app, git_root=ROOT):
    """Return (short_version, build, commit, tag) from the app's Info.plist and git HEAD."""
    with (app / "Contents" / "Info.plist").open("rb") as source:
        info = plistlib.load(source)
    short_version = str(info.get("CFBundleShortVersionString") or "0.0.0")
    build = str(info.get("CFBundleVersion") or "0")
    commit = _git_revision(git_root)
    return short_version, build, commit, f"{short_version}-{build}-{commit}"


def _stage_dsym(installed_binary, source_binary, destination):
    """Archive a matching `.dSYM` bundle for one shipped Mach-O binary."""
    if not installed_binary.is_file():
        raise RuntimeError(f"missing shipped binary: {installed_binary}")
    if has_embedded_dwarf(installed_binary):
        raise RuntimeError(f"{installed_binary}: shipped binary contains embedded __DWARF")
    expected_uuids = macho_uuids(installed_binary)
    candidate_dsym = Path(f"{source_binary}.dSYM")
    if candidate_dsym.is_dir():
        if macho_uuids(candidate_dsym) != expected_uuids:
            raise RuntimeError(f"{candidate_dsym} UUID does not match {installed_binary}")
        shutil.copytree(candidate_dsym, destination)
    else:
        if not source_binary.is_file():
            raise RuntimeError(f"missing source binary for dsymutil: {source_binary}")
        run(["xcrun", "dsymutil", str(source_binary), "-o", str(destination)])
        if macho_uuids(destination) != expected_uuids:
            raise RuntimeError(f"{destination} UUID does not match {installed_binary}")
    return expected_uuids


def archive_dsyms(app, output_dir, targets, git_root=ROOT, tag=None):
    """Archive packed dSYMs outside the bundle and verify every shipped binary matches."""
    embedded_dsyms = sorted(p.relative_to(app).as_posix() for p in app.rglob("*.dSYM"))
    if embedded_dsyms:
        raise RuntimeError(f"{app}: bundle must not contain .dSYM directories: {embedded_dsyms}")
    short_version, build, commit, computed_tag = version_tag(app, git_root=git_root)
    resolved_tag = tag or computed_tag
    output_dir.mkdir(parents=True, exist_ok=True)
    archive_dir = output_dir / resolved_tag
    with tempfile.TemporaryDirectory(dir=output_dir) as staging_root:
        staged = Path(staging_root) / resolved_tag
        staged.mkdir(parents=True, exist_ok=True)
        recorded = []
        for app_rel, source_binary, dsym_name in targets:
            installed = app / app_rel
            dsym_path = staged / dsym_name
            uuids = _stage_dsym(installed, Path(source_binary), dsym_path)
            recorded.append(
                {
                    "app_path": Path(app_rel).as_posix(),
                    "dsym": dsym_name,
                    "uuids": [f"{uuid} ({arch})" for uuid, arch in uuids],
                }
            )
        manifest = {
            "version": short_version,
            "build": build,
            "commit": commit,
            "tag": resolved_tag,
            "binaries": recorded,
        }
        (staged / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        if archive_dir.exists():
            shutil.rmtree(archive_dir)
        staged.rename(archive_dir)
    return archive_dir


def _dsym_map(dsym_dir):
    """Map executable basenames in a dSYM archive directory to `.dSYM` bundle paths."""
    mapping = {}
    manifest_path = dsym_dir / "manifest.json"
    if manifest_path.is_file():
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        for item in manifest.get("binaries", []):
            dsym_path = dsym_dir / item["dsym"]
            if not dsym_path.is_dir():
                continue
            app_name = Path(item["app_path"]).name
            mapping[app_name] = dsym_path
            if app_name == "echode-cli":
                mapping.setdefault("echode", dsym_path)
    for bundle in sorted(dsym_dir.glob("*.dSYM")):
        if bundle.is_dir():
            mapping.setdefault(bundle.stem, bundle)
            dwarf_dir = bundle / "Contents" / "Resources" / "DWARF"
            if dwarf_dir.is_dir():
                for binary in dwarf_dir.iterdir():
                    stem = re.sub(r"-[0-9a-f]{16}$", "", binary.name)
                    mapping.setdefault(binary.name, bundle)
                    mapping.setdefault(stem, bundle)
    return mapping


def _atos_resolve(dsym_path, load_address, addresses):
    completed = subprocess.run(
        ["xcrun", "atos", "-o", str(dsym_path), "-l", load_address, *addresses],
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode != 0:
        return {}
    lines = completed.stdout.splitlines()
    if len(lines) != len(addresses):
        return {}
    resolved = {}
    for address, output in zip(addresses, lines):
        stripped = output.strip()
        if stripped and stripped != address:
            resolved[address] = stripped
    return resolved


def symbolicate_panic_log(dsym_dir, panic_log):
    """Resolve `panic.ndjson` frame addresses against archived `.dSYM` bundles."""
    dsyms = _dsym_map(dsym_dir)
    reports = []
    for raw_line in panic_log.read_text(encoding="utf-8").splitlines():
        if not raw_line.strip():
            continue
        entry = json.loads(raw_line)
        backtrace = entry.get("backtrace")
        if not backtrace:
            reports.append(f"[{entry.get('source', 'unknown')}] {entry.get('message', '')}")
            continue
        addresses = []
        for match in re.finditer(r"^\s*\d+:\s+(0x[0-9a-fA-F]+)\b", backtrace, re.MULTILINE):
            address = match.group(1)
            if address not in addresses:
                addresses.append(address)
        resolved = {}
        for image in entry.get("images", []):
            image_name = Path(image.get("path", "")).name
            load = image.get("load")
            dsym_path = dsyms.get(image_name)
            if dsym_path and load and addresses:
                for addr, line in _atos_resolve(dsym_path, load, addresses).items():
                    resolved.setdefault(addr, line)
        out_lines = [f"[{entry.get('source', 'thread')} thread={entry.get('thread')}] {entry.get('message', '')}"]
        for line in backtrace.splitlines():
            out_lines.append(line)
            match = re.match(r"^\s*\d+:\s+(0x[0-9a-fA-F]+)\b", line)
            if match and match.group(1) in resolved:
                out_lines.append(f"      => {resolved[match.group(1)]}")
        reports.append("\n".join(out_lines))
    return "\n\n".join(reports)


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

    archived = sub.add_parser("archive-dsyms")
    archived.add_argument("--app", type=Path, required=True)
    archived.add_argument("--output-dir", type=Path, required=True)
    archived.add_argument(
        "--target",
        nargs=3,
        action="append",
        required=True,
        metavar=("APP_PATH", "SOURCE_BIN", "DSYM_NAME"),
    )
    archived.add_argument("--tag")

    symbolicated = sub.add_parser("symbolicate")
    symbolicated.add_argument("--dsyms", type=Path, required=True)
    symbolicated.add_argument("--panic-log", type=Path, required=True)

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
    elif args.command == "archive-dsyms":
        print(archive_dsyms(args.app, args.output_dir, args.target, tag=args.tag))
    elif args.command == "symbolicate":
        output = symbolicate_panic_log(args.dsyms, args.panic_log)
        if output:
            print(output)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main(sys.argv[1:]))
    except RuntimeError as error:
        print(error, file=sys.stderr)
        raise SystemExit(1) from error
