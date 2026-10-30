#!/usr/bin/env python3
"""Hold source-changing tool calls until an in-progress verification releases its lock."""

import fcntl
import json
from pathlib import Path
import sys
import time

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
LOCK = ROOT / "target/verification/.lock"
# The policy is the budget source; leave time to return JSON before the host kills the hook.
DEFAULT_TIMEOUT = json.loads((ROOT / "scripts/verification.json").read_text())["hook_timeout"] - 10
MUTATING = {"Shell", "Write", "StrReplace", "Delete", "EditNotebook", "Task"}


def wait_for_lock(lock_path, timeout):
    """Return once `lock_path` is not exclusively held. A missing file is idle."""
    if not lock_path.exists():
        return True
    deadline = time.monotonic() + timeout
    with lock_path.open("a") as lock:
        while True:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                fcntl.flock(lock, fcntl.LOCK_UN)
                return True
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    return False
                time.sleep(0.05)


def respond(event, root=ROOT, timeout=DEFAULT_TIMEOUT):
    if not isinstance(event, dict):
        raise ValueError("preToolUse input must be a JSON object")
    tool = event.get("tool_name") or event.get("tool") or ""
    if tool not in MUTATING:
        return {"permission": "allow"}
    if wait_for_lock(root / "target/verification/.lock", timeout):
        return {"permission": "allow"}
    return {
        "permission": "deny",
        "user_message": "検査がまだ終わっていないため、ファイルを変える操作を止めました。",
        "agent_message": "target/verification/.lock が検査で使われています。ロックが外れてから編集してください。",
    }


def main():
    try:
        response = respond(json.load(sys.stdin))
    except (OSError, ValueError) as error:
        response = {
            "permission": "deny",
            "user_message": "検査の完了待ちを始められませんでした。",
            "agent_message": str(error),
        }
    print(json.dumps(response, ensure_ascii=False))


if __name__ == "__main__":
    main()
