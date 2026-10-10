#!/usr/bin/env python3
"""Select checks from one policy and keep evidence tied to the source contents."""

import argparse
from contextlib import contextmanager
import fcntl
import fnmatch
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time

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
        if not set(check["areas"]).issubset(policy["areas"]):
            raise ValueError("unknown verification area")
    return policy


def matches(changed, patterns):
    return [path for path in changed if any(fnmatch.fnmatchcase(path, pattern) for pattern in patterns)]


def select_checks(policy, changed, all_checks=False):
    selected = {}
    global_hits = matches(changed, policy["global_inputs"])
    infrastructure = matches(changed, policy["infrastructure"])
    for check in policy["checks"]:
        hits = sorted(set(path for area in check["areas"] for path in matches(changed, policy["areas"][area])))
        reasons = global_hits + hits
        if check["mode"] == "automatic":
            reasons += infrastructure
        if all_checks or reasons or (changed and check.get("always")):
            selected[check["id"]] = sorted(set(reasons)) or ["all checks" if all_checks else "every change"]
    return selected


def tool_identity(root):
    # Validation results must not silently survive a compiler/PATH change. Failed
    # version queries are also recorded; a selected missing tool will fail the check.
    identity = {key: os.environ.get(key, "") for key in (
        "PATH", "RUSTUP_TOOLCHAIN", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS",
        "MIRIFLAGS", "ASAN_OPTIONS", "TSAN_OPTIONS", "RTSAN_ENABLE", "SDKROOT", "DEVELOPER_DIR",
        "CARGO_TARGET_DIR", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_TARGET",
        "CC", "CFLAGS", "MACOSX_DEPLOYMENT_TARGET", "MAKEFLAGS",
    )}
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
            result = subprocess.run(command, cwd=root, capture_output=True, timeout=15)
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


def load_report(directory, stamp):
    try:
        report = json.loads((directory / "report.json").read_text())
        if report.get("version") == 1 and report.get("fingerprint") == stamp:
            return report
    except (OSError, ValueError):
        pass
    return {"version": 1, "fingerprint": stamp, "results": {}}


def save_report(directory, report, policy, selected):
    report["updated_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    temporary = directory / "report.json.tmp"
    temporary.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    temporary.replace(directory / "report.json")
    lines = ["# 変更に対する検査", "", f"更新: {report['updated_at']}",
             f"ソースの識別値: `{report['fingerprint']}`", "", "## 選ばれた検査", ""]
    for check in policy["checks"]:
        name = check["id"]
        if name not in selected:
            continue
        result = report["results"].get(name, {"status": "pending"})
        lines.append(f"- `{name}`: **{result['status']}** — {check['purpose']}")
        if result.get("reason"):
            lines.append(f"  - 未実行の理由: {result['reason']}")
        if result.get("log"):
            lines.append(f"  - ログ: [{name}]({name}.log)")
        lines.append("  - 選択の根拠: " + ", ".join(selected[name]))
    lines += ["", "## 対象の変更", ""] + [f"- `{path}`" for path in report["changed_paths"]]
    lines += ["", "passed はこの識別値のソースだけに有効。deferred は合格ではなく、理由を記録した未実行。", ""]
    temporary = directory / "report.md.tmp"
    temporary.write_text("\n".join(lines))
    temporary.replace(directory / "report.md")


def run_check(root, check, log):
    started = time.monotonic()
    with log.open("w") as output:
        output.write(f"Command: make {check['id']}\nTimeout: {check['timeout']} s\n\n")
        output.flush()
        try:
            process = subprocess.Popen(
                ["make", check["id"]], cwd=root, stdout=output, stderr=subprocess.STDOUT,
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
            print(f"{name} [{checks[name]['mode']}]: {checks[name]['purpose']}")
            print("  " + ", ".join(reasons))
        if not selected:
            print("対象の変更はありません。コミット済みの変更には --base、全検査には --all を指定します。")
        return 0
    directory = root / "target/verification"
    with report_lock(directory):
        tools = tool_identity(root)
        stamp = fingerprint(root, changed, args.base, tools)
        report = load_report(directory, stamp)
        report.update({"changed_paths": changed, "base": args.base, "selected": selected, "tools": tools})
        for name in args.defer:
            report["results"][name] = {"status": "deferred", "reason": args.reason.strip()}
        save_report(directory, report, policy, selected)
        for check in policy["checks"]:
            name = check["id"]
            if name not in requested:
                continue
            previous = report["results"].get(name, {}).get("status")
            if not args.force and previous == "passed":
                print(f"{name}: passed (ソースが同じため前回の結果を使用)", flush=True)
                continue
            if check["mode"] == "manual" and name not in args.run:
                continue
            print(f"{name}: 実行中", flush=True)
            report["results"][name] = {"status": "running"}
            save_report(directory, report, policy, selected)
            result = run_check(root, check, directory / f"{name}.log")
            report["results"][name] = result
            # Concurrent edits may occur while an agent or CI runs a long check.
            if changed_paths(root, args.base) != changed or fingerprint(root, changed, args.base, tool_identity(root)) != stamp:
                result["status"] = "stale"
                result["error"] = "source or toolchain changed during verification"
                save_report(directory, report, policy, selected)
                print(f"{name}: 検査中に変更がありました。再実行が必要です。", file=sys.stderr)
                return 1
            save_report(directory, report, policy, selected)
            print(f"{name}: {result['status']} ({name}.log)", flush=True)
            if result["status"] != "passed":
                return 1
        if changed_paths(root, args.base) != changed or fingerprint(root, changed, args.base, tool_identity(root)) != stamp:
            print("検査の確認中に変更がありました。make verify を再実行してください。", file=sys.stderr)
            return 1
        pending = [name for name in requested if report["results"].get(name, {}).get("status") not in ("passed", "deferred")]
        print("結果: target/verification/report.md", flush=True)
        if pending:
            print("未実行: " + ", ".join(sorted(pending)), file=sys.stderr)
            print("計測は --run ID で実行するか、--defer ID --reason 理由 で未実行の理由を記録してください。", file=sys.stderr)
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
