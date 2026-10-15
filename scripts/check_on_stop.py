#!/usr/bin/env python3
"""Adapt the stop event's JSON protocol to the shared verification runner."""

import json
from pathlib import Path
import subprocess
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent


def respond(event, invoke):
    if not isinstance(event, dict):
        raise ValueError("stop input must be a JSON object")
    if event.get("status") != "completed":
        return {}
    if invoke() == 0:
        return {}
    return {"followup_message": (
        "必要な検査が失敗したか、有効な結果を確認できませんでした。target/agent-check.log と "
        "target/verification/report.md と target/verification/product-contracts.json を読み、失敗を直してください。"
        "必須の回帰テストを削除・無視したり、0件実行を合格にして終えてはいけません。"
        "その後 make verify を実行してください。"
        "実機・計測を実行しない場合は scripts/verify.py --defer ID --reason 理由で記録してください。"
    )}


def invoke():
    (ROOT / "target").mkdir(exist_ok=True)
    with (ROOT / "target/agent-check.log").open("w") as log:
        return subprocess.run([sys.executable, str(ROOT / "scripts/verify.py")],
                              cwd=ROOT, stdout=log, stderr=subprocess.STDOUT).returncode


def main():
    try:
        response = respond(json.load(sys.stdin), invoke)
    except (OSError, ValueError) as error:
        response = {"followup_message": "終了時の検査を開始できませんでした。入力と検査環境を確認し、make verify を実行してください。原因: " + str(error)}
    print(json.dumps(response, ensure_ascii=False))


if __name__ == "__main__":
    main()
