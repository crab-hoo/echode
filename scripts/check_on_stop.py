#!/usr/bin/env python3
"""Adapt the stop event's JSON protocol to the shared verification runner."""

import argparse
import json
from pathlib import Path
import subprocess
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
# Same bound as `loop_limit` in .cursor/hooks.json: a failure another session caused
# in a shared tree must not keep one agent from ever stopping.
LOOP_LIMIT = 3
FOLLOWUP = (
    "必要な検査が失敗したか、有効な結果を確認できませんでした。target/agent-check.log と "
    "target/verification/report.md と target/verification/product-contracts.json を読み、失敗を直してください。"
    "必須の回帰テストを削除・無視したり、0件実行を合格にして終えてはいけません。"
    "その後 make verify を実行してください。"
    "実機・計測を実行しない場合は scripts/verify.py --record-only --defer ID=理由 で検査ごとに理由を記録してください。"
)


def respond(event, invoke):
    if not isinstance(event, dict):
        raise ValueError("stop input must be a JSON object")
    if event.get("status") != "completed":
        return {}
    if invoke() == 0:
        return {}
    return {"followup_message": FOLLOWUP}


def respond_claude(event, invoke, counts=ROOT / "target/claude-stop-blocks.json"):
    """Claude Code's Stop event continues the agent with `decision: block`."""
    if not isinstance(event, dict):
        raise ValueError("stop input must be a JSON object")
    session = str(event.get("session_id") or "unknown")
    try:
        blocks = json.loads(counts.read_text())
    except (OSError, ValueError):
        blocks = {}
    if invoke() == 0:
        blocks.pop(session, None)
        response = {}
    else:
        blocks[session] = blocks.get(session, 0) + 1
        if blocks[session] > LOOP_LIMIT:
            blocks.pop(session, None)
            response = {"systemMessage": "終了時の検査が続けて失敗しています。" + FOLLOWUP}
        else:
            response = {"decision": "block", "reason": FOLLOWUP}
    counts.parent.mkdir(parents=True, exist_ok=True)
    counts.write_text(json.dumps(blocks))
    return response


def invoke():
    (ROOT / "target").mkdir(exist_ok=True)
    with (ROOT / "target/agent-check.log").open("w") as log:
        return subprocess.run([sys.executable, str(ROOT / "scripts/verify.py")],
                              cwd=ROOT, stdout=log, stderr=subprocess.STDOUT).returncode


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", choices=("cursor", "claude"), default="cursor")
    args = parser.parse_args(argv)
    try:
        event = json.load(sys.stdin)
        response = respond_claude(event, invoke) if args.host == "claude" else respond(event, invoke)
    except (OSError, ValueError) as error:
        message = "終了時の検査を開始できませんでした。入力と検査環境を確認し、make verify を実行してください。原因: " + str(error)
        response = {"decision": "block", "reason": message} if args.host == "claude" else {"followup_message": message}
    print(json.dumps(response, ensure_ascii=False))


if __name__ == "__main__":
    main()
