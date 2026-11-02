"""Validate durable documentation links without requiring old local audit files."""

import os
from pathlib import Path
import re
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
                errors.append(f"{document}: new target evidence link: {destination}; preserve a summary under docs/evidence/")
            elif LOCAL_EVIDENCE_NOTICE not in content:
                errors.append(f"{document}: legacy evidence needs a local-record notice")
            continue
        if not target.exists():
            errors.append(f"{path.relative_to(root)}: broken local link: {destination}")
    return errors
