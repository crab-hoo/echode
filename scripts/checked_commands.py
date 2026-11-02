#!/usr/bin/env python3
"""Run argv commands without a shell, retaining logs and aggregating every exit code.

This is a logging utility for focused development checks, not verification evidence.
Selection, caching, and completion remain the responsibility of verify.py.
"""

import argparse
import json
import os
from pathlib import Path
import re
import shlex
import signal
import subprocess
import sys
import time


def header(command, timeout):
    """The lines a log starts with, before anything the command printed."""
    return f"Command: {shlex.join(command)}\nTimeout: {timeout} s\n\n"


def run_logged(root, command, log, timeout, env=None):
    started = time.monotonic()
    log.parent.mkdir(parents=True, exist_ok=True)
    result = {"command": command, "log": str(log)}
    with log.open("w") as output:
        output.write(header(command, timeout))
        output.flush()
        try:
            process = subprocess.Popen(command, cwd=root, stdout=output, stderr=subprocess.STDOUT,
                                       start_new_session=True, env=env)
        except OSError as error:
            output.write(str(error) + "\n")
            result.update(exit_code=127, error=str(error))
        else:
            try:
                result["exit_code"] = process.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait()
                output.write("\nVERIFICATION TIMEOUT\n")
                result.update(exit_code=124, error="timeout")
    result["seconds"] = round(time.monotonic() - started, 3)
    result["status"] = "passed" if result["exit_code"] == 0 and not result.get("error") else "failed"
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--command", action="append", required=True, help="quoted argv, not shell syntax; repeat to run all")
    parser.add_argument("--log-dir", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=1200)
    parser.add_argument("--expect-exit", type=int)
    parser.add_argument("--expect-failure", help="regex for a specific assertion in a single expected-red command")
    args = parser.parse_args()
    if not (0 < args.timeout < float("inf")):
        parser.error("timeout must be positive and finite")
    expected = args.expect_exit is not None or args.expect_failure is not None
    if expected and (len(args.command) != 1 or not args.expect_exit or args.expect_exit < 0 or not args.expect_failure):
        parser.error("expected red requires one command, a positive --expect-exit and --expect-failure assertion")
    try:
        pattern = re.compile(args.expect_failure) if expected else None
        commands = [shlex.split(command) for command in args.command]
        if any(not command for command in commands):
            parser.error("empty command")
    except (ValueError, re.error) as error:
        parser.error(str(error))
    directory = args.log_dir.resolve()
    directory.mkdir(parents=True, exist_ok=True)
    results = []
    for number, command in enumerate(commands, 1):
        result = run_logged(Path.cwd(), command, directory / f"{number}.log", args.timeout,
                            {**os.environ, "RUSTUP_AUTO_INSTALL": "0"})
        if expected:
            # Only what the command printed counts: the header repeats its argv,
            # which holds the very pattern the assertion is matched with (ENV-06).
            printed = Path(result["log"]).read_text(errors="replace")
            printed = printed.removeprefix(header(command, args.timeout))
            result["expected_failure"] = bool(
                not result.get("error") and result["exit_code"] == args.expect_exit
                and pattern.search(printed))
            result["status"] = "expected_failure" if result["expected_failure"] else "failed"
        results.append(result)
        (directory / "commands.json").write_text(json.dumps(results, indent=2) + "\n")
        if result["status"] == "failed":
            print(f"command {number}: failed (exit {result['exit_code']}); {result['log']}", file=sys.stderr)
    return int(any(result["status"] == "failed" for result in results))


if __name__ == "__main__":
    sys.exit(main())
