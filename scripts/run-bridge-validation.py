#!/usr/bin/env python3
"""Bound hangs and require a real sanitizer diagnostic for positive controls."""

import argparse
from pathlib import Path
import os
import re
import signal
import subprocess
import sys

parser = argparse.ArgumentParser()
parser.add_argument("--log", type=Path, required=True)
parser.add_argument("--expect")
parser.add_argument("--site")
parser.add_argument("--timeout", type=int, default=120)
parser.add_argument("command", nargs=argparse.REMAINDER)
args = parser.parse_args()
if not args.command:
    parser.error("a command is required")
args.log.parent.mkdir(parents=True, exist_ok=True)
# The common runner already owns an isolated group. Do not detach grandchildren
# from it: its outer watchdog must be able to terminate the entire check.
supervised = os.environ.get("ECHODE_VERIFY_GROUP") == "1" and os.getsid(0) == os.getpgrp()
process = subprocess.Popen(args.command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=not supervised)
try:
    stdout, stderr = process.communicate(timeout=args.timeout)
except subprocess.TimeoutExpired as error:
    # Cargo may have a hung test child. Terminate this isolated process group,
    # rather than leaving that child alive with the capture pipes and file locks.
    if supervised:
        partial = "".join(chunk.decode(errors="replace") if isinstance(chunk, bytes) else (chunk or "")
                          for chunk in (error.stdout, error.stderr))
        args.log.write_text(partial + "\nVALIDATION TIMEOUT\n")
        print("bridge validation timed out: " + str(args.log), file=sys.stderr, flush=True)
        os.killpg(os.getpgrp(), signal.SIGKILL)
    else:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    stdout, stderr = process.communicate()
    args.log.write_text(stdout + stderr + "\nVALIDATION TIMEOUT\n")
    sys.exit("bridge validation did not complete; see " + str(args.log))
output = stdout + stderr
args.log.write_text(output)
if args.expect:
    # A crash, linker error, or timeout alone never proves the tool was active.
    if process.returncode == 0 or not re.search(args.expect, output) or (args.site and not re.search(args.site, output)):
        print(output[-8000:], file=sys.stderr)
        sys.exit("expected sanitizer diagnostic was absent: " + str(args.log))
    print("PASS sanitizer positive control: " + args.log.stem)
else:
    if process.returncode != 0:
        print(output[-8000:], file=sys.stderr)
        sys.exit("bridge validation failed: " + str(args.log))
    print(stdout, end="")
