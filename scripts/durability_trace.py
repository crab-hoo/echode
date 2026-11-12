#!/usr/bin/env python3
"""Trace which package files the product syncs before the journal forgets an edit.

Builds a small interposer for `fcntl(F_FULLFSYNC)`, `fsync` and renames, runs
`echode serve` on a copy of a fixture package, sends a few edits, and checks that
every renamed Git object, ref and media object has its directory synced before
the next journal write or compaction. Opens no window or audio device.
See docs/reviews/2026-11-02-fix-instructions.md (XCA-01, XCA-03).
"""

import argparse
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "crates/echode-history/tests/fixtures/schema-1"
WAV = ROOT / "crates/echode-cli/tests/fixtures/sine-pad-a4-128.wav"
WATCHED = (".git/objects/", ".git/refs/", "media/objects/")

INTERPOSER = r"""
#include <fcntl.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/param.h>
#include <unistd.h>
#define INTERPOSE(n, o) __attribute__((used)) static struct { const void *a; const void *b; } i_##o \
  __attribute__((section("__DATA,__interpose"))) = { (const void *)n, (const void *)o };
static void note(const char *what, const char *path) {
  const char *log = getenv("ECHODE_DURABILITY_LOG"); if (!log) return;
  int fd = open(log, O_WRONLY | O_APPEND | O_CREAT, 0600); if (fd < 0) return;
  char line[MAXPATHLEN + 64]; int n = snprintf(line, sizeof line, "%s %s\n", what, path ? path : "?");
  write(fd, line, n); close(fd);
}
static void note_fd(const char *what, int fd) { char p[MAXPATHLEN]; p[0] = 0; fcntl(fd, F_GETPATH, p); note(what, p); }
static int my_fcntl(int fd, int cmd, ...) { va_list a; va_start(a, cmd); long v = va_arg(a, long); va_end(a);
  if (cmd == F_FULLFSYNC) note_fd("SYNC", fd);
  return fcntl(fd, cmd, v); }
INTERPOSE(my_fcntl, fcntl)
static int my_fsync(int fd) { note_fd("SYNC", fd); return fsync(fd); }
INTERPOSE(my_fsync, fsync)
static int my_rename(const char *a, const char *b) { note("RENAME", b); return rename(a, b); }
INTERPOSE(my_rename, rename)
static int my_renameat(int fa, const char *a, int fb, const char *b) {
  char p[MAXPATHLEN]; p[0] = 0; fcntl(fb, F_GETPATH, p); strlcat(p, "/", sizeof p); strlcat(p, b, sizeof p);
  note("RENAME", p); return renameat(fa, a, fb, b); }
INTERPOSE(my_renameat, renameat)
"""


def events(text, package):
    """(kind, path relative to the package) for each traced call inside it."""
    found = []
    prefix = os.path.realpath(package) + "/"
    for line in text.splitlines():
        kind, _, path = line.partition(" ")
        path = os.path.realpath(path)
        if path == prefix.rstrip("/"):
            found.append((kind, "."))
        elif path.startswith(prefix):
            found.append((kind, path[len(prefix) :]))
    return found


def unsynced(found):
    """Watched renames whose directory was not synced before the next journal event."""
    pending, missing = {}, []
    for kind, path in found:
        journal = path == "journal.jsonl" or path.startswith(".journal.jsonl")
        if journal:
            missing.extend(pending.values())
            pending.clear()
        if kind == "RENAME" and path.startswith(WATCHED):
            pending[path] = path
        elif kind == "SYNC":
            for renamed in list(pending):
                if os.path.dirname(renamed) == (path if path != "." else ""):
                    pending.pop(renamed)
    return sorted(set(missing))


def trace(echode, directory):
    work = Path(directory)
    library = work / "trace.dylib"
    (work / "trace.c").write_text(INTERPOSER)
    subprocess.run(["clang", "-dynamiclib", "-O1", "-o", str(library), str(work / "trace.c")], check=True)
    package = work / "song.echode"
    shutil.copytree(FIXTURE, package)
    log = work / "trace.log"
    sock = work / "s.sock"
    environment = dict(os.environ, DYLD_INSERT_LIBRARIES=str(library), ECHODE_DURABILITY_LOG=str(log))
    server = subprocess.Popen(
        [str(echode), "serve", str(package), sock.name], cwd=work, env=environment, stderr=subprocess.PIPE, text=True
    )
    try:
        deadline = time.monotonic() + 10
        while not sock.exists():
            if server.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError(
                    "echode serve did not start: " + (server.stderr.read() if server.poll() is not None else "timeout")
                )
            time.sleep(0.05)
        client = socket.socket(socket.AF_UNIX)
        client.connect(str(sock))
        stream = client.makefile("rw")

        def call(method, params):
            stream.write(json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}) + "\n")
            stream.flush()
            reply = json.loads(stream.readline())
            if "error" in reply:
                raise RuntimeError(f"{method}: {reply['error']}")

        # Each request also lets the document thread finish and compact the previous save.
        call("actions.perform", {"type": "track.add", "name": "first"})
        time.sleep(1)
        call(
            "actions.transaction",
            {
                "actions": [
                    {"type": "track.add", "name": "audio"},
                    {"type": "audio.import", "track": {"ref": 0, "field": "id"}, "start": 0, "path": str(WAV)},
                ]
            },
        )
        time.sleep(1)
        call("actions.perform", {"type": "track.add", "name": "last"})
        time.sleep(1)
        call("actions.perform", {"type": "track.add", "name": "compacts the last"})
    finally:
        server.send_signal(signal.SIGKILL)
        server.wait()
    return log.read_text() if log.exists() else "", package


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--echode",
        type=Path,
        default=ROOT / "target/debug/echode",
        help="a built CLI; build it with cargo build -p echode-cli",
    )
    parser.add_argument(
        "--require-durable", action="store_true", help="exit 1 when a watched rename is not synced in time"
    )
    args = parser.parse_args()
    if not args.echode.is_file():
        sys.exit(f"{args.echode} is missing; run cargo build -p echode-cli")
    with tempfile.TemporaryDirectory(prefix="echode-durability-") as directory:
        text, package = trace(args.echode, directory)
        found = events(text, package)
    for kind, path in found:
        print(kind, path)
    missing = unsynced(found)
    print(f"\nunsynced before the next journal event: {len(missing)}")
    for path in missing:
        print("  " + path)
    return 1 if args.require_durable and missing else 0


if __name__ == "__main__":
    sys.exit(main())
