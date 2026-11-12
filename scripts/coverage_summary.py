#!/usr/bin/env python3
"""Summarize llvm-cov export JSON by Rust crate and Swift module (ADR 0290).

Usage: coverage_summary.py OUTPUT_DIRECTORY EXPORT.json...

Lines of product sources only: test directories, `*_tests.rs`, spikes,
examples and the crates that only serve tests are left out. Writes summary.md
and summary.json next to the inputs.
"""

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TEST_PARTS = {"tests", "Tests", "examples", "benches", "fuzz_targets"}
# Fuzz targets and the fault-injection plugin are test tools, not product.
TOOL_CRATES = {"echode-fuzz", "echode-test-plugin"}
# Code that runs in plugin helper processes. The host kills a helper right after
# its shutdown reply, before the helper writes its profile, so these count low.
HELPER_UNITS = ("echode-plugin-host", "echode-sine-pad")


def unit(path):
    """The crate or Swift module a product file belongs to, or None."""
    try:
        relative = Path(path).resolve().relative_to(ROOT)
    except ValueError:
        return None
    parts = relative.parts
    if set(parts) & TEST_PARTS or relative.name.endswith("_tests.rs") or parts[0] in ("spikes", "target"):
        return None
    if parts[0] == "crates" and len(parts) > 2 and parts[2] == "src":
        return None if parts[1] in TOOL_CRATES else parts[1]
    if parts[0] == "swift" and "Sources" in parts:
        index = parts.index("Sources")
        if index + 1 < len(parts) - 1:
            return "swift/" + parts[index + 1]
    return None


def summarize(exports):
    units = {}
    for export in exports:
        for data in json.loads(Path(export).read_text())["data"]:
            for entry in data["files"]:
                name = unit(entry["filename"])
                if name is None:
                    continue
                lines = entry["summary"]["lines"]
                total = units.setdefault(name, {"count": 0, "covered": 0, "files": 0})
                total["count"] += lines["count"]
                total["covered"] += lines["covered"]
                total["files"] += 1
    return dict(sorted(units.items()))


def percent(covered, count):
    return 100.0 * covered / count if count else 0.0


def render(units):
    rows = ["| 単位 | ファイル | 行 | 実行した行 | 割合 |", "|---|---:|---:|---:|---:|"]
    for language in ("Rust", "Swift"):
        selected = {name: total for name, total in units.items() if name.startswith("swift/") == (language == "Swift")}
        for name, total in selected.items():
            rows.append(
                f"| {name} | {total['files']} | {total['count']} | {total['covered']} | "
                f"{percent(total['covered'], total['count']):.1f}% |"
            )
        count = sum(total["count"] for total in selected.values())
        covered = sum(total["covered"] for total in selected.values())
        if selected:
            rows.append(
                f"| **{language} 計** | {sum(t['files'] for t in selected.values())} | {count} | {covered} | "
                f"{percent(covered, count):.1f}% |"
            )
    rows.append("")
    rows.append(f"{'・'.join(HELPER_UNITS)} の helper のプロセスで動く行は、強制終了で記録が残らず少なく数える。")
    return "\n".join(rows) + "\n"


def main(arguments):
    if len(arguments) < 2:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    output = Path(arguments[0])
    units = summarize(arguments[1:])
    if not units:
        print("coverage_summary: no product sources in the exports", file=sys.stderr)
        return 1
    output.mkdir(parents=True, exist_ok=True)
    (output / "summary.json").write_text(json.dumps(units, indent=2) + "\n")
    (output / "summary.md").write_text(render(units))
    print(render(units), end="")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
