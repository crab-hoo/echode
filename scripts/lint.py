#!/usr/bin/env python3
"""Format and lint the Swift, Python and shell sources (ADR 0323).

  lint.py setup                 install the pinned ruff, shellcheck and shfmt into target/lint-tools
  lint.py tools                 fail unless those pinned tools are installed
  lint.py check [--pinned-only] report formatting and lint findings without changing a file
  lint.py format                rewrite the files with the formatters

Rust keeps `cargo fmt` and clippy. swift-format comes with Xcode, whose version
the verification already records; `--pinned-only` leaves it out where another
Xcode runs (GitHub's hosted runners). Every tool keeps its own default rules.
"""

import argparse
import hashlib
import shutil
import subprocess
import sys
import venv
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REQUIREMENTS = ROOT / "scripts/lint-tools.txt"
TOOLS = ROOT / "target/lint-tools"
STAMP = TOOLS / "requirements.sha256"
PINNED = ("ruff", "shellcheck", "shfmt")
SWIFT_FORMAT = ["xcrun", "swift-format"]
# Four spaces and indented `case` labels, as the scripts were written.
SHFMT_STYLE = ["-i", "4", "-ci"]
SHELLS = {"sh", "bash"}
PYTHONS = {"python", "python3"}


def interpreter(first_line):
    """The program a `#!` line runs, without its directory or `env`."""
    if not first_line.startswith("#!"):
        return None
    words = first_line[2:].split()
    if words and Path(words[0]).name == "env":
        words = [word for word in words[1:] if not word.startswith("-")]
    return Path(words[0]).name if words else None


def language(path, first_line):
    if path.suffix == ".swift":
        return "swift"
    if path.suffix == ".py":
        return "python"
    if path.suffix in (".sh", ".bash"):
        return "shell"
    program = interpreter(first_line)
    if program in SHELLS:
        return "shell"
    if program in PYTHONS:
        return "python"
    return None


def first_line(path):
    with path.open("rb") as source:
        return source.readline(256).decode(errors="replace").rstrip("\r\n")


def sources(root):
    """Tracked and untracked, non-ignored files by language, as `make verify` sees the tree."""
    listed = subprocess.run(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        cwd=root,
        capture_output=True,
        check=True,
    ).stdout
    found = {"swift": [], "python": [], "shell": []}
    for name in sorted(set(filter(None, listed.decode().split("\0")))):
        path = root / name
        # A deleted but unstaged file is still listed; a symlink is checked where it points.
        if path.is_symlink() or not path.is_file():
            continue
        kind = language(path, first_line(path) if not path.suffix else "")
        if kind:
            found[kind].append(name)
    return found


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def binary(name, tools=TOOLS):
    return tools / "bin" / name


def missing_tools(requirements=REQUIREMENTS, tools=TOOLS):
    """Why the pinned tools cannot be used, or an empty list."""
    stamp = tools / STAMP.name
    try:
        installed = stamp.read_text().strip()
    except OSError:
        return [f"{tools.relative_to(ROOT) if tools.is_relative_to(ROOT) else tools} is not set up"]
    if installed != digest(requirements):
        return [f"{requirements.name} changed since the tools were installed"]
    return [name for name in PINNED if not binary(name, tools).is_file()]


def setup():
    """A fresh environment, so a removed or older pin cannot linger."""
    shutil.rmtree(TOOLS, ignore_errors=True)
    venv.create(TOOLS, with_pip=True)
    installed = subprocess.run(
        [
            str(TOOLS / "bin/python"),
            "-m",
            "pip",
            "install",
            "--quiet",
            "--disable-pip-version-check",
            "--require-hashes",
            "--only-binary",
            ":all:",
            "--no-deps",
            "-r",
            str(REQUIREMENTS),
        ],
        check=False,
    )
    if installed.returncode != 0:
        print("lint tools: pip could not install scripts/lint-tools.txt (network required)", file=sys.stderr)
        return 1
    STAMP.write_text(digest(REQUIREMENTS) + "\n")
    return report_tools()


def report_tools():
    problems = missing_tools()
    if problems:
        print("lint tools: " + "; ".join(problems) + ": run make setup-lint", file=sys.stderr)
        return 1
    for name in PINNED:
        result = subprocess.run([str(binary(name)), "--version"], capture_output=True, text=True, check=False)
        if result.returncode != 0:
            print(f"lint tools: {name} --version failed: run make setup-lint", file=sys.stderr)
            return 1
        version = next((line for line in result.stdout.splitlines() if "version" in line.lower()), result.stdout)
        # Flushed before the tools write to the same stream.
        print(f"{name}: {version.strip()}", flush=True)
    return 0


def commands(files, pinned_only):
    """Each check as (name, command); none of them changes a file."""
    ruff = str(binary("ruff"))
    found = []
    if files["swift"] and not pinned_only:
        found.append(
            (
                "swift-format",
                [*SWIFT_FORMAT, "lint", "--strict", "--parallel", "--configuration", ".swift-format", *files["swift"]],
            )
        )
    if files["python"]:
        found.append(("ruff format", [ruff, "format", "--check", *files["python"]]))
        found.append(("ruff check", [ruff, "check", "--no-fix", "--output-format", "concise", *files["python"]]))
    if files["shell"]:
        found.append(("shfmt", [str(binary("shfmt")), *SHFMT_STYLE, "--list", *files["shell"]]))
        found.append(("shellcheck", [str(binary("shellcheck")), "--format", "gcc", *files["shell"]]))
    return found


def check(pinned_only):
    if report_tools():
        return 1
    failed = []
    for name, command in commands(sources(ROOT), pinned_only):
        # Every tool runs, so one pass reports all findings.
        if subprocess.run(command, cwd=ROOT, check=False).returncode != 0:
            failed.append(name)
    if failed:
        print(f"check-lint: {', '.join(failed)} found problems.", file=sys.stderr)
        print("  Layout: make fmt. Lint findings: fix the code (ADR 0323).", file=sys.stderr)
    return int(bool(failed))


def format_sources():
    if report_tools():
        return 1
    files = sources(ROOT)
    steps = []
    if files["swift"]:
        swift = [*SWIFT_FORMAT, "format", "--in-place", "--parallel", "--configuration", ".swift-format"]
        # One pass is not always a fixed point: after a rule removes parentheses,
        # the printer can lay out the result again (ADR 0323). The build checks the outcome.
        steps += [swift + files["swift"], swift + files["swift"]]
    if files["python"]:
        ruff = str(binary("ruff"))
        steps += [[ruff, "check", "--fix", "--select", "I", *files["python"]], [ruff, "format", *files["python"]]]
    if files["shell"]:
        steps.append([str(binary("shfmt")), *SHFMT_STYLE, "--write", *files["shell"]])
    for command in steps:
        if subprocess.run(command, cwd=ROOT, check=False).returncode != 0:
            return 1
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("action", choices=("setup", "tools", "check", "format"))
    parser.add_argument("--pinned-only", action="store_true", help="skip swift-format, whose version Xcode decides")
    args = parser.parse_args()
    if args.pinned_only and args.action != "check":
        parser.error("--pinned-only applies to check")
    if args.action == "setup":
        return setup()
    if args.action == "tools":
        return report_tools()
    if args.action == "check":
        return check(args.pinned_only)
    return format_sources()


if __name__ == "__main__":
    sys.exit(main())
