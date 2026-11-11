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
import tsan_runtime
import verify

ROOT = Path(__file__).resolve().parent.parent
BUILD_FILES = ("Contents/MacOS/Echode", "Contents/MacOS/echode-plugin-host",
               "Contents/PlugIns/EchodeSinePad.clap/Contents/MacOS/EchodeSinePad",
               "Contents/PlugIns/EchodeSinePad.clap/Contents/Info.plist",
               "Contents/MacOS/echode-cli")
LOG_LIMIT = 2 * 1024 * 1024  # Diagnostic copies only; never a product capacity limit.
# The app's own diagnostic records (ADR 0143, 0238, 0322), never user projects.
APP_LOG_DIR = Path.home() / "Library/Logs/Echode"
APP_LOG_FILES = ("echode.ndjson", "echode.1.ndjson", "audition.ndjson", "audition.1.ndjson",
                 "panic.ndjson", "panic.1.ndjson")


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


def setup_needed():
    """Make targets whose prepared tools no longer match this machine.

    It is the judgment the TSan checks make before running, so a missing or
    outdated runtime is told apart from a source change that only needs make verify.
    """
    try:
        bridge_ready = tsan_runtime.prepared(tsan_runtime.context())
    except (OSError, subprocess.CalledProcessError):
        bridge_ready = False
    return [] if bridge_ready else ["setup-bridge"]


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


def collect(root, *, use_report_base=True):
    stamp = source_stamp(root)
    tools = verify.tool_identity(root)
    policy = verify.load_policy(root)
    contexts = verify.external_contexts(policy)
    report, error = read_json(root / "target/verification/report.json")
    if report and (report.get("version") not in (1, 2) or not isinstance(report.get("results"), dict)
                   or not all(isinstance(value, dict) for value in report["results"].values())):
        report, error = None, "invalid verification record"
    base = os.environ.get("VERIFY_BASE", (report or {}).get("base") if use_report_base else None)
    changed = verify.changed_paths(root, base)
    selected = verify.plan_checks(root, policy, changed, base)
    stamps = verify.check_fingerprints(root, policy, tools, contexts)
    busy = lock_busy(root)
    results = (report or {}).get("results", {})
    checks = []
    for check in policy["checks"]:
        name = check["id"]
        if name not in selected:
            continue
        previous = results.get(name, {})
        fresh = bool(report and report.get("version") == 2 and previous.get("fingerprint") == stamps[name]
                     and not (previous.get("status") == "passed" and check.get("device_context")
                              and contexts.get("audio-output") is None))
        checks.append({"id": name, "mode": check["mode"],
                       "status": previous.get("status", "not_run") if fresh else "stale" if previous else "not_run",
                       "fresh": fresh, "reason": previous.get("reason") if fresh else None,
                       "log": f"target/verification/{name}.log"})
    freshness = ("unavailable" if not report else "current" if all(check["fresh"] for check in checks)
                 else "partial" if any(check["fresh"] for check in checks) else "stale")
    full_tier = verify.unverified_full_tier(policy, results, stamps)
    tools_needed = ("git", "make", "rustc", "cargo", "swift", "xcodebuild", "xcrun", "cargo-deny", "cmake", "ninja")
    # Never export tools' raw environment values, Cargo config, or report.tools.
    versions = {key: value for key, value in tools.items()
                if key in ("rustc -vV", "cargo --version", "swift --version", "xcodebuild -version")
                or (key.startswith("rustc +") and key.endswith(" -vV"))}
    data = {"version": 1, "collected_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "source": stamp, "changed_paths": changed, "base": base,
            "environment": {"system": list(os.uname()), "versions": versions,
                            "tool_errors": [key for key, value in versions.items() if not value.startswith("0:")],
                            "missing_tools": [name for name in tools_needed if not shutil.which(name)],
                            "setup_needed": setup_needed()},
            "build": build_status(root, stamp),
            "verification": {"freshness": freshness,
                             "running": busy, "error": error, "checks": checks, "full_tier_unverified": full_tier,
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
    setup = data["environment"]["setup_needed"]
    lines = ["# Echode の診断", "", f"取得: {data['collected_at']}",
             f"ソース: `{data['source']}`", "",
             f"必要なツールの不足: {', '.join(missing) if missing else 'なし'}",
             f"ツールの起動エラー: {', '.join(errors) if errors else 'なし'}",
             f"準備のやり直し: {', '.join('make ' + target for target in setup) if setup else 'なし'}"
             + ("（TSan の検査専用ランタイムが未準備か、Xcode・CMake・Ninja の版と合わない）"
                if "setup-bridge" in setup else ""),
             f"開発用アプリ: {data['build']['status']}（ソースと実行ファイルの照合）",
             f"検査結果: {verification['freshness']} / 実行中: {verification['running']}", ""]
    for check in verification["checks"]:
        reason = f" — {check['reason']}" if check.get("reason") else ""
        lines.append(f"- {check['id']}: {check['status']}{reason}")
    if verification["full_tier_unverified"]:
        lines += ["", "全体検査で未確認（作業の完了条件ではない。main へのプッシュごとに GitHub の full が実行）: "
                  + ", ".join(verification["full_tier_unverified"])]
    lines += ["", "検査の詳細: target/verification/report.md",
              "アプリが stale / unknown なら make app。検査が stale / partial / unavailable なら make verify。",
              "検査中は編集せず、完了を待つ。failed は該当ログ、not_run は実行か理由の記録が必要。",
              "この診断は新しいテストを実行せず、表示中のアプリの版や聴感も確認しない。", ""]
    return "\n".join(lines)


def bundle(root, data):
    parent = root / "target/debug-reports"
    parent.mkdir(parents=True, exist_ok=True)
    destination = Path(tempfile.mkdtemp(prefix=time.strftime("%Y%m%dT%H%M%SZ-", time.gmtime()), dir=parent))
    candidates = [Path("target/agent-check.log"), Path("target/verification/product-contracts.json")]
    candidates += [Path("target/toolchains/tsan/manifest.json")]
    for pattern in ("thread/errno/*.log", "thread/control/*.log", "*/product/*.log"):
        candidates += sorted(path.relative_to(root) for path in (root / "target/bridge").glob(pattern))
    policy = verify.load_policy(root)
    candidates += [Path(f"target/verification/{check['id']}.log") for check in policy["checks"]]
    candidates += [Path(f"target/verification/preflight-{name}.log") for name in policy.get("preflights", {})]
    candidates += [Path("target/verification/failure-latency.json")]
    candidates += [Path("target/verification/timing-history.jsonl")]
    candidates += [Path(f"target/verification/product-contracts/{contract['id']}{suffix}.log")
                   for contract in policy.get("product_contracts", []) for suffix in ("", "-negative")]
    candidates += sorted(path.relative_to(root) for path in (root / "target").glob("spike-*.json"))
    copied = []
    for relative in candidates:
        source = root / relative
        # Do not follow links into user projects or external diagnostic files.
        if not source.is_file() or source.is_symlink() or not source.resolve().is_relative_to((root / "target").resolve()):
            continue
        copied.append(copy_tail(source, destination / relative.relative_to("target"), destination))
    app_logs = []
    for name in APP_LOG_FILES:
        source = APP_LOG_DIR / name
        if not source.is_file() or source.is_symlink():
            continue
        app_logs.append(copy_tail(source, destination / "app-logs" / name, destination))
    data = {**data, "artifacts": copied, "app_logs": app_logs,
            "artifact_note": "ログは過去の実行を含む調査資料。合格の有効性は verification の各 checks の fresh と status で判断する。"}
    (destination / "diagnosis.json").write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n")
    note = "\nログにはローカルのパスやプラグイン名が含まれ得る。共有前に内容を確認する。\n"
    if app_logs:
        note += ("app-logs/ はアプリが書いたログ（出力・ヘルパー・文書の出来事と、オーディオスレッドの取りこぼしの"
                 "ノード名。ADR 0322）、試聴の要求の記録（操作の種類と各段の時刻、素材のパス）、"
                 "Rust の panic の記録（場所・理由・スレッド。ADR 0238）。\n")
    (destination / "README.md").write_text(render(data) + note)
    return destination


def copy_tail(source, target, destination):
    target.parent.mkdir(parents=True, exist_ok=True)
    with source.open("rb") as input_file:
        size = os.fstat(input_file.fileno()).st_size
        input_file.seek(max(0, size - LOG_LIMIT))
        target.write_bytes(input_file.read(LOG_LIMIT))
    return {"path": str(target.relative_to(destination)), "truncated": size > LOG_LIMIT}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--json", action="store_true")
    mode.add_argument("--bundle", action="store_true")
    mode.add_argument("--record-build", action="store_true")
    mode.add_argument("--source-stamp", action="store_true")
    parser.add_argument("--expected-source")
    args = parser.parse_args()
    if args.record_build and not args.expected_source:
        parser.error("--record-build requires --expected-source")
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
