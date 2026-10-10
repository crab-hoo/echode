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
        "変更に必要な検査が完了していません。target/agent-check.log と "
        "target/verification/report.md を読み、失敗を直してください。"
        "未実行の計測は scripts/verify.py --run ID で実行するか、"
        "実機や作業範囲の理由を --defer ID --reason 理由 で記録してください。"
        "deferred は合格ではないので、最終報告にも未実行の検査と理由を書いてください。"
        "その後 make verify を実行してください。"
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
