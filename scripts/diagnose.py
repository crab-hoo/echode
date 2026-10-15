#!/usr/bin/env python3
"""Collect local diagnostics without opening the app, devices, or user projects."""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

sys.dont_write_bytecode = True
import verify

ROOT = Path(__file__).resolve().parent.parent
BUILD_FILES = ("Contents/MacOS/Echode", "Contents/MacOS/echode-plugin-host")
LOG_LIMIT = 2 * 1024 * 1024  # Diagnostic copies only; never a product capacity limit.


def read_json(path):
    try:
        value = json.loads(path.read_text())
        if not isinstance(value, dict):
            raise ValueError("expected a JSON object")
        return value, None
    except FileNotFoundError:
        return None, "missing"
    except (OSError, ValueError) as error:
        return None, str(error)


def source_stamp(root):
    # This identity intentionally excludes compiler/environment and diff base.
    # Verification has its own stricter source + tools fingerprint.
    return verify.fingerprint(root, [], None, {})


def file_hash(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def lock_busy(root):
    try:
        with (root / "target/verification/.lock").open("rb") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return True
            fcntl.flock(lock, fcntl.LOCK_UN)
    except FileNotFoundError:
        pass
    return False


def record_build(root, expected_source):
    app = root / "target/Echode.app"
    stamp = source_stamp(root)
    if stamp != expected_source:
        raise ValueError("source changed while building; run make app again")
    record = {"version": 1, "built_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
              "source": stamp, "files": {name: file_hash(app / name) for name in BUILD_FILES}}
    path = root / "target/Echode.build.json"
    temporary = path.with_suffix(".json.tmp")
    temporary.write_text(json.dumps(record, indent=2) + "\n")
    temporary.replace(path)


def build_status(root, stamp):
    record, error = read_json(root / "target/Echode.build.json")
    if error:
        return {"status": "unknown", "reason": error, "next": "make app"}
    if (record.get("version") != 1 or not isinstance(record.get("files"), dict)
            or set(record["files"]) != set(BUILD_FILES)):
        return {"status": "unknown", "reason": "invalid build record", "next": "make app"}
    try:
        intact = all(file_hash(root / "target/Echode.app" / name) == record["files"][name] for name in BUILD_FILES)
    except OSError:
        intact = False
    return {"status": "current" if intact and record.get("source") == stamp else "stale",
            "binaries_match": intact, "built_at": record.get("built_at"),
            "source_matches": record.get("source") == stamp, "next": "make app"}


def collect(root):
    stamp = source_stamp(root)
    tools = verify.tool_identity(root)
    policy = verify.load_policy(root)
    report, error = read_json(root / "target/verification/report.json")
    if report and (report.get("version") != 1 or not isinstance(report.get("results"), dict)
                   or not all(isinstance(value, dict) for value in report["results"].values())):
        report, error = None, "invalid verification record"
    base = os.environ.get("VERIFY_BASE", (report or {}).get("base"))
    changed = verify.changed_paths(root, base)
    selected = verify.select_checks(policy, changed)
    current = verify.fingerprint(root, changed, base, tools)
    busy = lock_busy(root)
    fresh = bool(report and report.get("version") == 1 and report.get("fingerprint") == current)
    results = (report or {}).get("results", {})
    checks = [{"id": check["id"], "mode": check["mode"],
               "status": results.get(check["id"], {}).get("status", "not_run") if fresh else "unknown",
               "reason": results.get(check["id"], {}).get("reason") if fresh else None,
               "log": f"target/verification/{check['id']}.log"}
              for check in policy["checks"] if check["id"] in selected]
    tools_needed = ("git", "make", "rustc", "cargo", "swift", "xcodebuild", "xcrun", "cargo-deny")
    # Never export tools' raw environment values, Cargo config, or report.tools.
    versions = {key: value for key, value in tools.items()
                if key in ("rustc -vV", "cargo --version", "swift --version", "xcodebuild -version")
                or (key.startswith("rustc +") and key.endswith(" -vV"))}
    data = {"version": 1, "collected_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "source": stamp, "changed_paths": changed, "base": base,
            "environment": {"system": list(os.uname()), "versions": versions,
                            "tool_errors": [key for key, value in versions.items() if not value.startswith("0:")],
                            "missing_tools": [name for name in tools_needed if not shutil.which(name)]},
            "build": build_status(root, stamp),
            "verification": {"freshness": "current" if fresh else "stale" if report else "unavailable",
                             "running": busy, "error": error, "checks": checks,
                             "report": "target/verification/report.md"}}
    if source_stamp(root) != stamp:
        data["verification"]["freshness"] = "changed_during_collection"
        for check in checks:
            check["status"] = "unknown"
        data["build"]["status"] = "unknown"
    return data


def render(data):
    verification = data["verification"]
    missing = data["environment"]["missing_tools"]
    errors = data["environment"]["tool_errors"]
    lines = ["# Echode の診断", "", f"取得: {data['collected_at']}",
             f"ソース: `{data['source']}`", "",
             f"必要なツールの不足: {', '.join(missing) if missing else 'なし'}",
             f"ツールの起動エラー: {', '.join(errors) if errors else 'なし'}",
             f"開発用アプリ: {data['build']['status']}（ソースと実行ファイルの照合）",
             f"検査結果: {verification['freshness']} / 実行中: {verification['running']}", ""]
    for check in verification["checks"]:
        reason = f" — {check['reason']}" if check.get("reason") else ""
        lines.append(f"- {check['id']}: {check['status']}{reason}")
    lines += ["", "検査の詳細: target/verification/report.md",
              "アプリが stale / unknown なら make app。検査が stale / unavailable なら make verify。",
              "検査中は編集せず、完了を待つ。failed は該当ログ、not_run は実行か理由の記録が必要。",
              "この診断は新しいテストを実行せず、表示中のアプリの版や聴感も確認しない。", ""]
    return "\n".join(lines)


def bundle(root, data):
    parent = root / "target/debug-reports"
    parent.mkdir(parents=True, exist_ok=True)
    destination = Path(tempfile.mkdtemp(prefix=time.strftime("%Y%m%dT%H%M%SZ-", time.gmtime()), dir=parent))
    candidates = [Path("target/agent-check.log"), Path("target/verification/product-contracts.json")]
    policy = verify.load_policy(root)
    candidates += [Path(f"target/verification/{check['id']}.log") for check in policy["checks"]]
    candidates += [Path(f"target/verification/product-contracts/{contract['id']}{suffix}.log")
                   for contract in policy.get("product_contracts", []) for suffix in ("", "-negative")]
    candidates += sorted(path.relative_to(root) for path in (root / "target").glob("spike-*.json"))
    copied = []
    for relative in candidates:
        source = root / relative
        # Do not follow links into user projects or external diagnostic files.
        if not source.is_file() or source.is_symlink() or not source.resolve().is_relative_to((root / "target").resolve()):
            continue
        target = destination / relative.relative_to("target")
        target.parent.mkdir(parents=True, exist_ok=True)
        with source.open("rb") as input_file:
            size = os.fstat(input_file.fileno()).st_size
            input_file.seek(max(0, size - LOG_LIMIT))
            target.write_bytes(input_file.read(LOG_LIMIT))
        copied.append({"path": str(target.relative_to(destination)), "truncated": size > LOG_LIMIT})
    data = {**data, "artifacts": copied,
            "artifact_note": "ログは過去の実行を含む調査資料。合格の有効性は verification.freshness で判断する。"}
    (destination / "diagnosis.json").write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n")
    (destination / "README.md").write_text(render(data) + "\nログにはローカルのパスやプラグイン名が含まれ得る。共有前に内容を確認する。\n")
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--json", action="store_true")
    mode.add_argument("--bundle", action="store_true")
    mode.add_argument("--record-build", action="store_true")
    mode.add_argument("--source-stamp", action="store_true")
    parser.add_argument("--expected-source")
    args = parser.parse_args()
    try:
        if args.record_build:
            record_build(ROOT, args.expected_source)
            return 0
        if args.source_stamp:
            print(source_stamp(ROOT))
            return 0
        data = collect(ROOT)
        if args.bundle:
            print(bundle(ROOT, data))
        else:
            print(json.dumps(data, ensure_ascii=False, indent=2) if args.json else render(data))
        return 0
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print("診断を作成できませんでした: " + str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
