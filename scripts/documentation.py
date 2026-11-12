"""Validate durable documentation links without requiring old local audit files."""

import os
import re
from pathlib import Path
from urllib.parse import unquote, urlsplit

LOCAL_EVIDENCE_NOTICE = "当時のローカル記録"


def local_link_errors(root: Path, path: Path, content: str, legacy: dict) -> list[str]:
    """Return link diagnostics; legacy entries are scoped to the citing document."""
    errors = []
    destinations = re.findall(r"\]\(([^)]+)\)", content)
    destinations += re.findall(r"^\s*\[[^\]]+\]:\s*<?([^\s>]+)", content, re.MULTILINE)
    destinations += re.findall(r"<((?:\.\.?/|/|target/)[^<>\s]+)>", content)
    for destination in destinations:
        if destination.lower().startswith("file:"):
            # A file URL names one machine's path and is never checked (ENV-07).
            errors.append(f"{path.relative_to(root)}: file URL link: {destination}; use a relative link")
            continue
        if "://" in destination or destination.startswith("#"):
            continue
        relative = unquote(urlsplit(destination.split()[0].strip("<>")).path)
        target = Path(os.path.abspath(path.parent / relative))
        if target.is_relative_to(root / "target"):
            document = path.relative_to(root).as_posix()
            evidence = target.relative_to(root).as_posix()
            if evidence not in legacy.get(document, []):
                errors.append(
                    f"{document}: new target evidence link: {destination}; preserve a summary under docs/evidence/"
                )
            elif LOCAL_EVIDENCE_NOTICE not in content:
                errors.append(f"{document}: legacy evidence needs a local-record notice")
            continue
        if not target.exists():
            errors.append(f"{path.relative_to(root)}: broken local link: {destination}")
    return errors


def headings(text):
    """Index ATX headings, ignoring fenced examples; retain nested sections."""
    result = []
    fence = None
    lines = text.splitlines(keepends=True)
    for number, line in enumerate(lines, 1):
        marker = re.match(r"^ {0,3}(`{3,}|~{3,})(.*)$", line)
        if fence:
            if marker and marker[1][0] == fence[0] and len(marker[1]) >= fence[1] and not marker[2].strip():
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
        item["end"] = next(
            (other["start"] - 1 for other in result[index + 1 :] if other["level"] <= item["level"]), len(lines)
        )
    return result


def heading_anchors(text: str) -> set[str]:
    """GitHub-style anchors of the ATX headings outside code fences; repeats get -1, -2."""
    anchors, seen = set(), {}
    for item in headings(text):
        title = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", item["title"]).replace("`", "")
        base = re.sub(r"[^\w\- ]", "", title.strip().lower()).replace(" ", "-")
        count = seen.get(base, 0)
        anchors.add(base if count == 0 else f"{base}-{count}")
        seen[base] = count + 1
    return anchors


def anchor_errors(root: Path, path: Path, content: str) -> list[str]:
    """Return links whose #fragment names no heading of a local Markdown file."""
    errors = []
    cache = {}
    for destination in re.findall(r"\]\(([^)\s]+)\)", content):
        if "://" in destination or "#" not in destination:
            continue
        relative, fragment = destination.split("#", 1)
        target = Path(os.path.abspath(path.parent / unquote(relative))) if relative else path
        if relative and (target.suffix.lower() != ".md" or not target.is_file()):
            continue
        if target not in cache:
            cache[target] = heading_anchors(content if target == path else target.read_text(encoding="utf-8"))
        if unquote(fragment) not in cache[target]:
            errors.append(f"{path.relative_to(root)}: missing heading anchor: {destination}")
    return errors
