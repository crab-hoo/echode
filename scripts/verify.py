#!/usr/bin/env python3
"""Select checks from one policy and keep evidence tied to the source contents."""

import argparse
from contextlib import contextmanager
import fcntl
import fnmatch
import hashlib
import json
import math
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import tomllib

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
POLICY = Path("scripts/verification.json")


def git(root, *arguments):
    return subprocess.check_output(["git", *arguments], cwd=root, stderr=subprocess.PIPE)


def paths(output):
    return [os.fsdecode(item) for item in output.split(b"\0") if item]


def changed_paths(root, base=None):
    # --no-renames includes both the removed path and the destination. Comparing
    # to HEAD/base includes staged and unstaged edits; untracked files need ls-files.
    reference = base or "HEAD"
    if not base and subprocess.run(
        ["git", "rev-parse", "--verify", "HEAD"], cwd=root,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    ).returncode:
        changed = paths(git(root, "ls-files", "-z", "--cached"))
    else:
        commit = git(root, "rev-parse", "--verify", "--end-of-options", f"{reference}^{{commit}}").strip().decode()
        changed = paths(git(root, "diff", "--name-only", "--no-renames", "-z", commit, "--"))
    changed += paths(git(root, "ls-files", "-z", "--others", "--exclude-standard"))
    return sorted(set(changed))


def load_policy(root):
    policy = json.loads((root / POLICY).read_text())
    if policy.get("version") != 1:
        raise ValueError("unsupported verification policy")
    seen = set()
    for check in policy["checks"]:
        name = check["id"]
        if not re.fullmatch(r"[a-z][a-z0-9-]*", name) or name in seen:
            raise ValueError("invalid/duplicate check id")
        seen.add(name)
        if check["mode"] not in ("automatic", "manual") or check["timeout"] <= 0:
            raise ValueError("invalid mode/timeout")
        if not re.fullmatch(r"[a-z][a-z0-9-]*", check.get("make_target", name)):
            raise ValueError("invalid make target")
        if not set(check["areas"]).issubset(policy["areas"]):
            raise ValueError("unknown verification area")
        for field in ("inputs", "packages", "policy_inputs"):
            values = check.get(field, [])
            if not isinstance(values, list) or not all(isinstance(value, str) and value for value in values):
                raise ValueError("invalid verification " + field)
        if not (check.get("inputs") or check.get("packages")):
            raise ValueError("each check needs explicit inputs or packages")
        if any(key not in policy for key in check.get("policy_inputs", [])):
            raise ValueError("unknown verification policy input")
        if check.get("extended") and check["mode"] != "manual":
            raise ValueError("extended checks must be manual")
    return policy


def matches(changed, patterns):
    return [path for path in changed if any(fnmatch.fnmatchcase(path, pattern) for pattern in patterns)]


def select_checks(policy, changed, all_checks=False):
    selected = {}
    global_hits = matches(changed, policy["global_inputs"])
    infrastructure = matches(changed, policy["infrastructure"])
    for check in policy["checks"]:
        hits = sorted(set(path for area in check["areas"] for path in matches(changed, policy["areas"][area])))
        reasons = hits
        if check["mode"] == "automatic":
            reasons += global_hits + infrastructure
        if all_checks or (not check.get("extended") and (reasons or (changed and check.get("always")))):
            selected[check["id"]] = sorted(set(reasons)) or ["all checks" if all_checks else "every change"]
    return selected


def make_flags_identity(value):
    # GNU make escapes whitespace with backslashes; quotes are literal here.
    # Selection/deferral arguments cannot affect a build and must not invalidate
    # evidence when the next invocation is plain `make verify` or `make doctor`.
    tokens = [re.sub(r"\\(.)", r"\1", token) for token in re.findall(r"(?:\\.|[^\s\\])+", value)]
    return [token for token in tokens if token != "--" and not token.startswith("VERIFY_ARGS=")]


def tool_identity(root):
    # Validation results must not silently survive a compiler/PATH change. Failed
    # version queries are also recorded; a selected missing tool will fail the check.
    identity = {key: os.environ.get(key, "") for key in (
        "PATH", "RUSTUP_TOOLCHAIN", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS",
        "MIRIFLAGS", "ASAN_OPTIONS", "TSAN_OPTIONS", "RTSAN_ENABLE", "SDKROOT", "DEVELOPER_DIR",
        "CARGO_TARGET_DIR", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_TARGET",
        "CC", "CFLAGS", "MACOSX_DEPLOYMENT_TARGET", "MAKEFLAGS",
    )}
    identity["MAKEFLAGS"] = make_flags_identity(identity["MAKEFLAGS"])
    identity["python"] = sys.version + ":" + sys.executable
    identity["system"] = list(os.uname())
    cargo_config = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")))
    for filename in ("config", "config.toml"):
        path = cargo_config / filename
        if path.is_file():
            # Config can contain private registry information; record only its hash.
            identity["cargo " + filename] = hashlib.sha256(path.read_bytes()).hexdigest()
    commands = [["rustc", "-vV"], ["cargo", "--version"], ["swift", "--version"], ["xcodebuild", "-version"]]
    env_file = root / "scripts/bridge-toolchain.env"
    if env_file.exists():
        nightly = re.search(r"^BRIDGE_NIGHTLY=([a-z0-9-]+)$", env_file.read_text(), re.MULTILINE)
        if nightly:
            commands.append(["rustc", "+" + nightly[1], "-vV"])
    for command in commands:
        try:
            result = subprocess.run(command, cwd=root, capture_output=True, timeout=15,
                                    env={**os.environ, "RUSTUP_AUTO_INSTALL": "0"})
            identity[" ".join(command)] = str(result.returncode) + ":" + os.fsdecode(result.stdout + result.stderr)
        except (OSError, subprocess.TimeoutExpired) as error:
            identity[" ".join(command)] = str(error)
    return identity


def fingerprint(root, changed, base, tools):
    digest = hashlib.sha256()
    digest.update(json.dumps([changed, base, tools], sort_keys=True).encode())
    # Hash the whole non-ignored tree, including unchanged dependencies and fixtures.
    # Status/mtime alone cannot invalidate a result after another edit of the same file.
    files = paths(git(root, "ls-files", "-z", "--cached", "--others", "--exclude-standard"))
    for relative in sorted(set(files)):
        path = root / relative
        digest.update(os.fsencode(relative) + b"\0")
        if path.is_symlink():
            digest.update(b"link:" + os.fsencode(os.readlink(path)))
        elif path.is_file():
            digest.update(str(path.stat().st_mode).encode() + b"\0")
            with path.open("rb") as source:
                for block in iter(lambda: source.read(1024 * 1024), b""):
                    digest.update(block)
        else:
            digest.update(b"missing")
        digest.update(b"\0")
    return digest.hexdigest()


def package_directories(root, names):
    """Include local dependencies for all features/targets, including test/build dependencies."""
    if not names:
        return set()
    workspace = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]
    manifests = {}
    for pattern in workspace["members"]:
        for folder in root.glob(pattern):
            relative = folder.relative_to(root).as_posix()
            if any(fnmatch.fnmatchcase(relative, excluded) for excluded in workspace.get("exclude", [])):
                continue
            manifest = tomllib.loads((folder / "Cargo.toml").read_text())
            manifests[manifest["package"]["name"]] = folder
    requested = set(manifests) if names == ["*"] else set(names)
    if requested - manifests.keys():
        raise ValueError("unknown verification package: " + ", ".join(sorted(requested - manifests.keys())))
    pending = [manifests[name] for name in requested]
    visited = set()
    while pending:
        folder = pending.pop().resolve()
        if not folder.is_relative_to(root.resolve()):
            raise ValueError("verification cannot cache a local dependency outside the repository")
        if folder in visited:
            continue
        visited.add(folder)
        manifest = tomllib.loads((folder / "Cargo.toml").read_text())
        sections = [manifest, *manifest.get("target", {}).values()]
        for section in sections:
            for kind in ("dependencies", "dev-dependencies", "build-dependencies"):
                for alias, dependency in section.get(kind, {}).items():
                    base = folder
                    if isinstance(dependency, dict) and dependency.get("workspace"):
                        dependency = workspace.get("dependencies", {})[alias]
                        base = root
                    if isinstance(dependency, dict) and "path" in dependency:
                        pending.append(base / dependency["path"])
    return {folder.relative_to(root.resolve()).as_posix() for folder in visited}


def audio_output_identity():
    """Read device metadata without opening an IO stream; retain only a hash."""
    try:
        completed = subprocess.run(["system_profiler", "-json", "SPAudioDataType", "-detailLevel", "mini"],
                                   capture_output=True, text=True, timeout=10, check=True)
        sections = json.loads(completed.stdout)["SPAudioDataType"]
        devices = [device for section in sections if isinstance(section, dict) for device in section.get("_items", [])
                   if isinstance(device, dict) and device.get("coreaudio_default_audio_output_device") == "spaudio_yes"]
        if len(devices) != 1:
            return None
        rate = float(devices[0].get("coreaudio_device_srate", 0))
        if not math.isfinite(rate) or rate <= 0:
            return None
        fields = ("_name", "coreaudio_device_srate", "coreaudio_device_output",
                  "coreaudio_device_manufacturer", "coreaudio_device_transport", "coreaudio_output_source")
        description = {key: devices[0].get(key) for key in fields}
        return hashlib.sha256(json.dumps(description, sort_keys=True).encode()).hexdigest()
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
        return None


def external_contexts(policy):
    return {"audio-output": audio_output_identity()} if any(c.get("device_context") for c in policy["checks"]) else {}


def check_fingerprints(root, policy, tools, contexts=None):
    if contexts is None:
        contexts = external_contexts(policy)
    files = sorted(set(paths(git(root, "ls-files", "-z", "--cached", "--others", "--exclude-standard"))))
    common = policy["global_inputs"] + policy.get("evidence_inputs", ["Makefile", "scripts/verify.py", ".cargo/**"])
    contents = {}
    stamps = {}
    for check in policy["checks"]:
        patterns = common + check.get("inputs", [])
        folders = package_directories(root, check.get("packages", []))
        configuration = [check, common, tools, contexts if check.get("device_context") else {},
                         {key: policy[key] for key in check.get("policy_inputs", [])}]
        digest = hashlib.sha256(json.dumps(configuration, sort_keys=True).encode())
        for relative in files:
            if not (matches([relative], patterns)
                    or any(relative.startswith(folder + "/") for folder in folders)):
                continue
            if relative not in contents:
                path = root / relative
                value = hashlib.sha256()
                if path.is_symlink():
                    value.update(b"link:" + os.fsencode(os.readlink(path)))
                    path = path.resolve()
                    if not path.is_relative_to(root.resolve()) or not path.is_file():
                        raise ValueError("verification input symlinks must resolve to files inside the repository")
                if path.is_file():
                    value.update(str(path.stat().st_mode).encode() + b"\0")
                    with path.open("rb") as source:
                        for block in iter(lambda: source.read(1024 * 1024), b""):
                            value.update(block)
                else:
                    value.update(b"missing")
                contents[relative] = value.digest()
            digest.update(os.fsencode(relative) + b"\0" + contents[relative])
        stamps[check["id"]] = digest.hexdigest()
    return stamps


@contextmanager
def report_lock(directory):
    directory.mkdir(parents=True, exist_ok=True)
    with (directory / ".lock").open("a") as lock:
        deadline = time.monotonic() + 30
        while True:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise ValueError("another verification run holds target/verification/.lock")
                time.sleep(0.1)
        try:
            yield
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def load_report(directory):
    try:
        report = json.loads((directory / "report.json").read_text())
        if (isinstance(report, dict) and report.get("version") == 2 and isinstance(report.get("results"), dict)
                and all(isinstance(value, dict) and isinstance(value.get("fingerprint"), str)
                        for value in report["results"].values())):
            return report
    except (OSError, ValueError):
        pass
    # Whole-tree v1 evidence cannot prove the new per-check dependency identities.
    return {"version": 2, "results": {}}


def save_report(directory, report, policy, selected):
    report["updated_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    temporary = directory / "report.json.tmp"
    temporary.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    temporary.replace(directory / "report.json")
    lines = ["# 変更に対する検査", "", f"更新: {report['updated_at']}",
             f"ソースの識別値: `{report['fingerprint']}`", "", "## 選ばれた検査", ""]
    for check in policy["checks"]:
        name = check["id"]
        additional = check.get("extended") and name in report["results"]
        if name not in selected and not additional:
            continue
        kind = "自動検査" if check["mode"] == "automatic" else "追加の長時間検査" if check.get("extended") else "計測などの候補"
        result = report["results"][name]
        status = result["status"]
        if result.get("fingerprint") != report["input_fingerprints"][name]:
            status = "stale"
        if name not in selected:
            kind += "・通常の完了判定外"
        lines.append(f"- `{name}` ({kind}): **{status}** — {check['purpose']}")
        if result.get("reason"):
            lines.append(f"  - 未実行の理由: {result['reason']}")
        if result.get("log"):
            lines.append(f"  - ログ: [{name}]({name}.log)")
        lines.append(f"  - 検査入力の識別値: `{result['fingerprint']}`")
        lines.append("  - 選択の根拠: " + ", ".join(selected.get(name, ["以前に明示実行した追加検査の記録"])))
    lines += ["", "## 対象の変更", ""] + [f"- `{path}`" for path in report["changed_paths"]]
    lines += ["", "passed と deferred は各検査の入力・ツールが一致する間だけ有効。全体の識別値は変更監視用。not_run は実行か理由の記録が必要。deferred は合格ではない。", ""]
    temporary = directory / "report.md.tmp"
    temporary.write_text("\n".join(lines))
    temporary.replace(directory / "report.md")


def run_check(root, check, log):
    started = time.monotonic()
    with log.open("w") as output:
        target = check.get("make_target", check["id"])
        output.write(f"Command: make {target}\nTimeout: {check['timeout']} s\n\n")
        output.flush()
        try:
            process = subprocess.Popen(
                ["make", target], cwd=root, stdout=output, stderr=subprocess.STDOUT,
                start_new_session=True, env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1", "ECHODE_VERIFY_GROUP": "1"},
            )
        except OSError as error:
            output.write(str(error) + "\n")
            return {"status": "failed", "log": str(log), "error": str(error)}
        try:
            code = process.wait(timeout=check["timeout"])
        except subprocess.TimeoutExpired:
            # Stop Cargo's test children too; a hung process must not keep holding locks.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()
            output.write("\nVERIFICATION TIMEOUT\n")
            return {"status": "failed", "log": str(log), "error": "timeout"}
    return {"status": "passed" if code == 0 else "failed", "exit_code": code,
            "seconds": round(time.monotonic() - started, 3), "log": str(log)}


def verify(root, args):
    policy = load_policy(root)
    changed = changed_paths(root, args.base)
    selected = select_checks(policy, changed, args.all)
    checks = {check["id"]: check for check in policy["checks"]}
    for name in args.run:
        if name in checks and checks[name].get("extended"):
            selected[name] = ["explicit --run"]
    for name in args.only + args.run + args.defer:
        if name not in selected:
            raise ValueError(f"{name} is not selected; use --all to select an otherwise unrelated check")
    for name in args.run + args.defer:
        if checks[name]["mode"] != "manual":
            raise ValueError("--run/--defer accepts only manual measurements")
    if args.defer and not (args.reason and args.reason.strip()):
        raise ValueError("--defer requires a nonempty --reason")
    if set(args.run) & set(args.defer):
        raise ValueError("cannot run and defer the same check")
    requested = set(args.only or selected)
    if args.only and not set(args.run + args.defer).issubset(requested):
        raise ValueError("--run/--defer must be included in --only")
    if args.plan:
        for name, reasons in selected.items():
            kind = "自動検査" if checks[name]["mode"] == "automatic" else "追加の長時間検査" if checks[name].get("extended") else "計測などの候補"
            print(f"{name} [{kind}]: {checks[name]['purpose']}")
            print("  " + ", ".join(reasons))
        if not selected:
            print("対象の変更はありません。コミット済みの変更には --base、全検査には --all を指定します。")
        return 0
    directory = root / "target/verification"
    with report_lock(directory):
        tools = tool_identity(root)
        stamp = fingerprint(root, changed, args.base, tools)
        contexts = external_contexts(policy)
        stamps = check_fingerprints(root, policy, tools, contexts)
        report = load_report(directory)
        report.update({"fingerprint": stamp, "input_fingerprints": stamps, "changed_paths": changed,
                       "base": args.base, "selected": selected, "tools": tools})
        for name in selected:
            status = "pending" if checks[name]["mode"] == "automatic" else "not_run"
            previous = report["results"].get(name, {})
            if (previous.get("fingerprint") != stamps[name]
                    or (previous.get("status") == "passed" and checks[name].get("device_context")
                        and contexts.get("audio-output") is None)):
                report["results"][name] = {"status": status, "fingerprint": stamps[name]}
        for name in args.defer:
            report["results"][name] = {"status": "deferred", "reason": args.reason.strip(), "fingerprint": stamps[name]}
        save_report(directory, report, policy, selected)
        for check in policy["checks"]:
            name = check["id"]
            if name not in requested:
                continue
            previous = report["results"].get(name, {}).get("status")
            if not args.force and previous == "passed":
                print(f"{name}: passed (検査の入力・ツールが同じため前回の結果を使用)", flush=True)
                continue
            if check["mode"] == "manual" and name not in args.run:
                continue
            print(f"{name}: 実行中", flush=True)
            report["results"][name] = {"status": "running", "fingerprint": stamps[name]}
            save_report(directory, report, policy, selected)
            result = run_check(root, check, directory / f"{name}.log")
            if result["status"] == "passed" and check.get("device_context") and contexts.get("audio-output") is None:
                result.update({"status": "failed", "error": "audio device metadata unavailable; cannot attest this measurement"})
            result["fingerprint"] = stamps[name]
            report["results"][name] = result
            # Concurrent edits may occur while an agent or CI runs a long check.
            current_tools = tool_identity(root)
            if (changed_paths(root, args.base) != changed
                    or fingerprint(root, changed, args.base, current_tools) != stamp
                    or check_fingerprints(root, policy, current_tools)[name] != stamps[name]):
                result["status"] = "stale"
                result["error"] = "source or toolchain changed during verification"
                save_report(directory, report, policy, selected)
                print(f"{name}: 検査中に変更がありました。再実行が必要です。", file=sys.stderr)
                return 1
            save_report(directory, report, policy, selected)
            print(f"{name}: {result['status']} ({name}.log)", flush=True)
            if result["status"] != "passed":
                return 1
        current_tools = tool_identity(root)
        current_stamps = check_fingerprints(root, policy, current_tools)
        if (changed_paths(root, args.base) != changed or fingerprint(root, changed, args.base, current_tools) != stamp
                or any(current_stamps[name] != stamps[name] for name in selected)):
            print("検査の確認中に変更がありました。make verify を再実行してください。", file=sys.stderr)
            return 1
        # Never open hardware automatically. Every selected manual check needs
        # either explicit execution or an explanation attached to this source.
        pending = [name for name in requested
                   if report["results"][name]["status"] not in ("passed", "deferred")]
        print("結果: target/verification/report.md", flush=True)
        if pending:
            print("完了していない検査: " + ", ".join(sorted(pending)), file=sys.stderr)
            print("失敗はログを確認して修正してください。実機・計測は --run ID、未実行は --defer ID --reason 理由を使います。", file=sys.stderr)
            return 1
        return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default=os.environ.get("VERIFY_BASE"))
    parser.add_argument("--all", action="store_true")
    parser.add_argument("--only", action="append", default=[])
    parser.add_argument("--run", action="append", default=[])
    parser.add_argument("--defer", action="append", default=[])
    parser.add_argument("--reason")
    parser.add_argument("--plan", action="store_true")
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--github-matrix", action="store_true")
    args = parser.parse_args()
    try:
        if args.github_matrix:
            automatic = [check["id"] for check in load_policy(ROOT)["checks"] if check["mode"] == "automatic"]
            matrix = json.dumps(automatic)
            with Path(os.environ["GITHUB_OUTPUT"]).open("a") as output:
                output.write("checks=" + matrix + "\n")
            print(matrix)
            return 0
        return verify(ROOT, args)
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print("検査の選択または実行に失敗しました: " + str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
