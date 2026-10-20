#!/usr/bin/env python3
"""Collect agent context once, or read exact, bounded source excerpts."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
import diagnose
import verify

ROOT = Path(__file__).resolve().parent.parent
SNAPSHOT = "target/agent-context.json"
# Presentation budget only; callers may explicitly request a larger excerpt.
DEFAULT_MAX_BYTES = 6000
COMMON_GUIDES = ("AGENTS.md", "docs/README.md", "docs/status.md", "docs/development.md",
                 "docs/verification.md", "docs/debugging.md", "docs/PLAN.md", "docs/ARCHITECTURE.md",
                 "docs/agent-workflow.md")
# Only documentation references live here. Source globs and check selection stay
# in verification.json; this index cannot remove verification obligations.
AREA_GUIDES = {
    "bridge": ("docs/architecture/ui-and-metal.md", "docs/architecture/api-qa-and-distribution.md"),
    "audio": ("docs/architecture/audio-engine.md",),
    "isolation": ("docs/architecture/plugins-and-devices.md",),
    "drawing": ("docs/architecture/ui-and-metal.md",),
    "history": ("docs/architecture/model-and-history.md",),
    "input": ("docs/architecture/ui-and-metal.md",),
    "render": ("docs/architecture/audio-engine.md", "docs/architecture/api-qa-and-distribution.md"),
    "dependencies": ("docs/PLAN.md", "docs/architecture/api-qa-and-distribution.md"),
    "app_drawing": ("docs/architecture/ui-and-metal.md",),
    "product_audio": ("docs/architecture/audio-engine.md",),
}


def read_source(root, name, expected=None):
    path = (root / name).resolve(strict=True)
    if not path.is_relative_to(root.resolve()) or not path.is_file():
        raise ValueError("source must be a file inside the repository")
    raw = path.read_bytes()
    stamp = hashlib.sha256(raw).hexdigest()
    if expected and stamp != expected:
        raise ValueError("source changed; refresh the outline before using its line numbers")
    return {"path": path.relative_to(root.resolve()).as_posix(), "sha256": stamp,
            "bytes": len(raw)}, raw.decode("utf-8")


def headings(text):
    """Index ATX headings, ignoring fenced examples; retain nested sections."""
    result = []
    fence = None
    lines = text.splitlines(keepends=True)
    for number, line in enumerate(lines, 1):
        marker = re.match(r"^ {0,3}(`{3,}|~{3,})(.*)$", line)
        if fence:
            if (marker and marker[1][0] == fence[0] and len(marker[1]) >= fence[1]
                    and not marker[2].strip()):
                fence = None
            continue
        if marker:
            fence = (marker[1][0], len(marker[1]))
            continue
        match = re.match(r"^ {0,3}(#{1,6})[ \t]+(.+?)\s*$", line)
        if match:
            title = re.sub(r"[ \t]+#+[ \t]*$", "", match[2])
            result.append({"title": title, "level": len(match[1]), "start": number})
    for index, item in enumerate(result):
        item["end"] = next((other["start"] - 1 for other in result[index + 1:]
                            if other["level"] <= item["level"]), len(lines))
    return result


def source_view(root, name, *, outline=False, section=None, line_range=None,
                expected=None, max_bytes=DEFAULT_MAX_BYTES):
    if max_bytes <= 0:
        raise ValueError("--max-bytes must be positive")
    source, text = read_source(root, name, expected)
    lines = text.splitlines(keepends=True)
    entries = headings(text) if Path(name).suffix.lower() == ".md" else []
    if outline:
        return {"source": source, "total_lines": len(lines), "headings": entries}
    start, end = 1, len(lines)
    if section is not None:
        found = [item for item in entries if item["title"] == section]
        if len(found) != 1:
            raise ValueError("section is missing or ambiguous; use --outline and --lines START:END")
        start, end = found[0]["start"], found[0]["end"]
    if line_range is not None:
        if not re.fullmatch(r"[1-9][0-9]*:[1-9][0-9]*", line_range):
            raise ValueError("--lines requires START:END with positive line numbers")
        start, end = map(int, line_range.split(":"))
        if start > end or end > len(lines):
            raise ValueError("requested lines are outside the source")
    content = "".join(lines[start - 1:end])
    if len(content.encode("utf-8")) > max_bytes:
        raise ValueError("excerpt exceeds --max-bytes; use --outline, narrower --lines, or a larger explicit budget; no content was returned")
    return {"source": source, "total_lines": len(lines), "start": start, "end": end, "content": content}


def collect(root, requested_areas=()):
    # Match a new verify-plan invocation, rather than inheriting an old report's
    # diff base. Existing per-check evidence is still evaluated by diagnose.
    data = diagnose.collect(root, use_report_base=False)
    policy = verify.load_policy(root)
    if set(AREA_GUIDES) != set(policy["areas"]):
        raise ValueError("documentation areas differ from verification.json; update AREA_GUIDES")
    if not set(requested_areas).issubset(policy["areas"]):
        raise ValueError("unknown area; choose from " + ", ".join(sorted(policy["areas"])))
    changed = data["changed_paths"]
    areas = sorted(set(requested_areas) | {area for area, patterns in policy["areas"].items()
                                         if verify.matches(changed, patterns)})
    guides = dict.fromkeys([*COMMON_GUIDES, *(name for area in areas for name in AREA_GUIDES[area])])
    records = []
    for name in guides:
        source, text = read_source(root, name)
        records.append({**source, "headings": headings(text)})
    checks = {check["id"]: check for check in policy["checks"]}
    selection = {name: {"purpose": checks[name]["purpose"], "reasons": reasons}
                 for name, reasons in verify.select_checks(policy, changed).items()}
    if (data["verification"]["freshness"] == "changed_during_collection"
            or diagnose.source_stamp(root) != data["source"]):
        raise ValueError("source changed during context collection; collect again")
    return {"version": 1, "diagnosis": data, "areas": areas, "selection": selection, "guides": records}


def render_view(data, *, as_json=False, max_bytes=DEFAULT_MAX_BYTES):
    if as_json:
        output = json.dumps(data, ensure_ascii=False, indent=2) + "\n"
    else:
        source = data["source"]
        span = "outline" if "headings" in data else f"{data['start']}:{data['end']}"
        header = f"{source['path']}:{span} sha256={source['sha256']} ({data['total_lines']} lines)\n"
        if "headings" in data:
            body = "\n".join(f"{item['start']}:{item['end']} {'#' * item['level']} {item['title']}"
                             for item in data["headings"])
            output = header + (body or "No ATX Markdown headings; use --read --lines START:END or search symbols with rg.") + "\n"
        else:
            # Preserve the original text without per-line prefixes inflating
            # many short lines or changing code copied from the excerpt.
            output = header + data["content"]
    if len(output.encode("utf-8")) > max_bytes:
        raise ValueError("formatted view exceeds --max-bytes; narrow the range or use a larger explicit budget; no content was returned")
    return output


def render(data):
    diagnosis = data["diagnosis"]
    environment = diagnosis["environment"]
    verification = diagnosis["verification"]
    changes = diagnosis["changed_paths"]
    lines = [f"Source: {diagnosis['source']}",
             f"Environment: missing={environment['missing_tools']}; tool_errors={environment['tool_errors']}",
             f"App: {diagnosis['build']['status']}; verification: {verification['freshness']}; lock: {'BUSY - wait before editing' if verification['running'] else 'idle'}"]
    if verification["error"]:
        lines.append(f"Verification record error: {verification['error']}")
    lines += [f"Changes ({len(changes)}): {verify.summarize_reasons(changes, limit=5)}",
              "Areas (guide hints only): " + (", ".join(data["areas"]) or "common guides; inspect the audit index for the intended task")]
    automatic = [check for check in verification["checks"] if check["mode"] == "automatic"]
    passed = [check["id"] for check in automatic if check["status"] == "passed" and check["fresh"]]
    lines.append(f"Automatic: {len(passed)}/{len(automatic)} current passes" + (": " + ", ".join(passed) if passed else ""))
    for check in automatic:
        if check["id"] not in passed:
            lines.append(f"  {check['id']}: {check['status']} (fresh={check['fresh']}); {check['log']}")
    groups = {}
    for check in verification["checks"]:
        if check["mode"] == "manual":
            key = (check["status"], check["fresh"], check.get("reason") or "")
            groups.setdefault(key, []).append(check["id"])
    for (status, fresh, reason), names in groups.items():
        warning = "INVALID deferral: missing reason" if status == "deferred" and not reason.strip() else reason
        lines.append(f"Manual {status} (fresh={fresh}): {', '.join(names)}" + (f" — {warning}" if warning else ""))
    lines.append("Guides (AGENTS.md remains mandatory; follow related ADR/audit links):")
    lines.extend("  " + guide["path"] for guide in data["guides"])
    lines += [f"Full paths, selection reasons, guide hashes/outlines and logs: {SNAPSHOT}",
              "Inspect: python3 -B scripts/agent_context.py --outline PATH; --read PATH --section TITLE or --lines START:END",
              "Diagnostics only; completion still requires make verify. Refresh context after source changes."]
    return "\n".join(lines) + "\n"


def save(root, data):
    path = root / SNAPSHOT
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=path.parent, delete=False) as output:
            temporary = Path(output.name)
            json.dump(data, output, ensure_ascii=False, indent=2)
            output.write("\n")
        temporary.replace(path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--outline", metavar="PATH")
    modes.add_argument("--read", metavar="PATH")
    ranges = parser.add_mutually_exclusive_group()
    ranges.add_argument("--section", metavar="TITLE")
    ranges.add_argument("--lines", metavar="START:END")
    parser.add_argument("--expect-sha256")
    parser.add_argument("--max-bytes", type=int, default=DEFAULT_MAX_BYTES,
                        help="source-view output budget, including metadata; refuses oversized output")
    parser.add_argument("--area", action="append", default=[])
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    if (args.section is not None or args.lines is not None) and not args.read:
        parser.error("--section/--lines requires --read")
    if args.area and (args.read or args.outline):
        parser.error("--area is only for context collection")
    if args.expect_sha256 and not (args.read or args.outline):
        parser.error("--expect-sha256 requires --read/--outline")
    try:
        if args.read or args.outline:
            data = source_view(ROOT, args.read or args.outline, outline=bool(args.outline),
                               section=args.section, line_range=args.lines, expected=args.expect_sha256,
                               max_bytes=args.max_bytes)
            print(render_view(data, as_json=args.json, max_bytes=args.max_bytes), end="")
        else:
            data = collect(ROOT, args.area)
            save(ROOT, data)
            print(json.dumps(data, ensure_ascii=False, indent=2) if args.json else render(data), end="\n" if args.json else "")
        return 0
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print("Agent context unavailable: " + str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
