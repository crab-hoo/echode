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


ADR_DIR_REL = Path("docs/decisions")
ADR_INDEX_REL = Path("docs/decisions/README.md")
ADR_STATUSES = ("現行", "一部置換", "置換済み", "草案")
ADR_ACTIVE_STATUSES = ("現行", "一部置換")
ADR_INACTIVE_STATUSES = ("置換済み", "草案")
ADR_DOMAINS = (
    "全体方針・計画",
    "モデル・履歴・保存・API",
    "音声エンジン・書き出し",
    "プラグイン・隔離",
    "UI・描画・入力",
    "ビルド・検査・開発基盤",
)
ADR_CHUNK_SIZE = 40

_ADR_FILENAME_RE = re.compile(r"^(\d{4})-[a-z0-9-]+\.md$")
_ADR_TITLE_RE = re.compile(r"^#\s+(?:ADR\s+)?(\d{4})(?:[:.]\s*|\s+)(.+?)\s*$")
_ADR_STATUS_LINE_RE = re.compile(r"^-\s+状態:\s*(.+?)\s*$")
_ADR_DOMAIN_LINE_RE = re.compile(r"^-\s+領域:\s*(.+?)\s*$")
_ADR_STATUS_TOKEN_RE = re.compile(r"^(現行|一部置換|置換済み|草案)(?:$|[（(。．.:\s])")


def _extract_adr_header(path: Path, lines: list[str]) -> tuple[dict | None, list[str]]:
    errors = []
    match_name = _ADR_FILENAME_RE.match(path.name)
    if not match_name:
        return None, [f"{path.name}: ADR filename must match NNNN-slug.md"]
    file_num = match_name.group(1)

    title_line = next((line for line in lines if line.strip()), "")
    match_title = _ADR_TITLE_RE.match(title_line)
    if not match_title:
        errors.append(f"{path.name}: first heading must start with '# ADR {file_num}:' or '# {file_num}.'")
        title = title_line.lstrip("# ").strip()
    else:
        if match_title.group(1) != file_num:
            errors.append(f"{path.name}: heading ADR number {match_title.group(1)} does not match filename {file_num}")
        title = match_title.group(2).strip()

    header_lines = []
    started = False
    for line in lines:
        if not started:
            if line == title_line:
                started = True
            continue
        if line.startswith("## ") or line.strip() == "---":
            break
        header_lines.append(line)

    status_matches = [m.group(1) for line in header_lines if (m := _ADR_STATUS_LINE_RE.match(line))]
    domain_matches = [m.group(1) for line in header_lines if (m := _ADR_DOMAIN_LINE_RE.match(line))]

    if len(status_matches) != 1:
        errors.append(f"{path.name}: expected exactly 1 '- 状態:' header line, found {len(status_matches)}")
        status_raw, status = "", ""
    else:
        status_raw = status_matches[0]
        tok = _ADR_STATUS_TOKEN_RE.match(status_raw)
        if not tok:
            errors.append(f"{path.name}: '- 状態:' must start with one of {', '.join(ADR_STATUSES)}: {status_raw}")
            status = ""
        else:
            status = tok.group(1)

    if len(domain_matches) != 1:
        errors.append(f"{path.name}: expected exactly 1 '- 領域:' header line, found {len(domain_matches)}")
        domain = ""
    else:
        domain = domain_matches[0]
        if domain not in ADR_DOMAINS:
            errors.append(f"{path.name}: unknown '- 領域:' value '{domain}'")

    if errors:
        return None, errors

    links = re.findall(r"\[([^\]]+)\]\(([^)\s]+)\)", status_raw)
    all_adr_links = []
    for _label, dest in links:
        dest_file = dest.split("#", 1)[0]
        if _ADR_FILENAME_RE.match(dest_file):
            all_adr_links.append(dest_file)

    replacing_links = []
    paren_match = re.match(r"^(?:一部置換|置換済み|草案)（([^）]+)）", status_raw)
    if paren_match:
        for _label, dest in re.findall(r"\[([^\]]+)\]\(([^)\s]+)\)", paren_match.group(1)):
            dest_file = dest.split("#", 1)[0]
            if _ADR_FILENAME_RE.match(dest_file):
                replacing_links.append(dest_file)

    return {
        "number": file_num,
        "filename": path.name,
        "title": title,
        "status": status,
        "status_raw": status_raw,
        "domain": domain,
        "adr_links": all_adr_links,
        "replacing_links": replacing_links,
    }, []


def scan_adrs(root: Path) -> tuple[list[dict], list[str]]:
    """Parse and validate metadata headers for all ADR files under docs/decisions."""
    adr_dir = root / ADR_DIR_REL
    if not adr_dir.is_dir():
        return [], [f"missing ADR directory: {ADR_DIR_REL}"]

    files = sorted(p for p in adr_dir.glob("*.md") if p.name != "README.md")
    entries = []
    errors = []
    seen_numbers = {}
    for path in files:
        content = path.read_text(encoding="utf-8")
        entry, item_errors = _extract_adr_header(path, content.splitlines())
        errors.extend(item_errors)
        if entry is None:
            continue
        num = entry["number"]
        if num in seen_numbers:
            errors.append(f"{path.name}: duplicate ADR number {num} (already in {seen_numbers[num]})")
        else:
            seen_numbers[num] = path.name
        entries.append(entry)

    by_filename = {entry["filename"]: entry for entry in entries}
    for entry in entries:
        src_num = int(entry["number"])
        for target_file in entry["adr_links"]:
            if target_file not in by_filename:
                errors.append(f"{entry['filename']}: broken ADR link in '- 状態:': {target_file}")
        if entry["status"] in ("一部置換", "置換済み") and not entry["replacing_links"]:
            errors.append(
                f"{entry['filename']}: status '{entry['status']}' must list replacing ADR links in '状態: {entry['status']}（...）'"
            )
        for target_file in entry["replacing_links"]:
            if target_file in by_filename:
                target_num = int(by_filename[target_file]["number"])
                if target_num <= src_num:
                    errors.append(
                        f"{entry['filename']}: replacing ADR link {target_file} must have a higher number than {entry['number']}"
                    )

    return entries, errors


def _format_status_cell(entry: dict) -> str:
    status = entry["status"]
    if entry["replacing_links"]:
        seen = []
        for dest in entry["replacing_links"]:
            num = dest[:4]
            link = f"[{num}]({dest})"
            if link not in seen:
                seen.append(link)
        return f"{status}（{'・'.join(seen)}）"
    return status


def _escape_table_cell(text: str) -> str:
    return text.replace("|", "\\|").replace("\n", " ")


def render_adr_index(root: Path) -> tuple[str, list[str]]:
    """Render docs/decisions/README.md grouped by domain and status, plus header errors."""
    entries, errors = scan_adrs(root)
    counts = {domain: {s: 0 for s in ADR_STATUSES} for domain in ADR_DOMAINS}
    by_domain = {domain: {True: [], False: []} for domain in ADR_DOMAINS}
    for entry in entries:
        domain = entry["domain"]
        counts[domain][entry["status"]] += 1
        is_active = entry["status"] in ADR_ACTIVE_STATUSES
        by_domain[domain][is_active].append(entry)

    lines = [
        "# ADR 索引",
        "",
        "<!-- scripts/documentation.py --write-adr-index で自動生成。直接編集せず、各 ADR の `- 状態:` / `- 領域:` を更新する。 -->",
        "",
        f"全 {len(entries)} 件の設計判断を領域別・状態別に並べる。日々の作業で現在有効な制約を確かめる時は「現行・一部置換」を見る。",
        "",
        "## 集計",
        "",
        "| 領域 | 合計 | 現行 | 一部置換 | 置換済み | 草案 |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    totals = {s: 0 for s in ADR_STATUSES}
    for domain in ADR_DOMAINS:
        c = counts[domain]
        for s in ADR_STATUSES:
            totals[s] += c[s]
        row_total = sum(c.values())
        anchor = re.sub(r"[^\w\- ]", "", domain.lower()).replace(" ", "-")
        lines.append(
            f"| [{domain}](#{anchor}) | {row_total} | {c['現行']} | {c['一部置換']} | {c['置換済み']} | {c['草案']} |"
        )
    lines.append(
        f"| 合計 | {len(entries)} | {totals['現行']} | {totals['一部置換']} | {totals['置換済み']} | {totals['草案']} |"
    )

    for domain in ADR_DOMAINS:
        lines.extend(["", f"## {domain}"])
        for label, is_active in (("現行・一部置換", True), ("置換済み・草案", False)):
            group = by_domain[domain][is_active]
            if not group:
                continue
            lines.extend(["", f"### {label}"])
            chunks = [group[i : i + ADR_CHUNK_SIZE] for i in range(0, len(group), ADR_CHUNK_SIZE)]
            for chunk in chunks:
                if len(chunks) > 1:
                    lines.extend(["", f"#### ADR {chunk[0]['number']}〜{chunk[-1]['number']}"])
                lines.extend(["", "| ADR | 状態 | 表題 |", "|---|---|---|"])
                for entry in chunk:
                    num = entry["number"]
                    adr_link = f"[{num}]({entry['filename']})"
                    status_cell = _format_status_cell(entry)
                    title_cell = _escape_table_cell(entry["title"])
                    lines.append(f"| {adr_link} | {status_cell} | {title_cell} |")

    lines.append("")
    return "\n".join(lines), errors


def adr_index_errors(root: Path) -> list[str]:
    """Validate all ADR headers and confirm docs/decisions/README.md matches render_adr_index."""
    expected, errors = render_adr_index(root)
    if errors:
        return errors
    index_path = root / ADR_INDEX_REL
    if not index_path.is_file():
        return [f"missing generated ADR index: {ADR_INDEX_REL}"]
    actual = index_path.read_text(encoding="utf-8")
    if actual != expected:
        return [
            f"{ADR_INDEX_REL}: stale generated ADR index; run python3 -B scripts/documentation.py --write-adr-index"
        ]
    return []


if __name__ == "__main__":
    import argparse
    import sys

    parser = argparse.ArgumentParser(description="Validate or regenerate the ADR index.")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--write-adr-index", action="store_true")
    group.add_argument("--check-adr-index", action="store_true")
    args = parser.parse_args()

    rendered, header_errors = render_adr_index(args.root)
    if header_errors:
        for err in header_errors:
            print(err, file=sys.stderr)
        sys.exit(1)

    if args.write_adr_index:
        (args.root / ADR_INDEX_REL).write_text(rendered, encoding="utf-8")
        sys.exit(0)

    errors = adr_index_errors(args.root)
    if errors:
        for err in errors:
            print(err, file=sys.stderr)
        sys.exit(1)
