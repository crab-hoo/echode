#!/usr/bin/env python3
"""Check handwritten Rust foreign declarations by source path and item name."""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# Keep strings as opaque tokens (including raw strings), so examples and ABI
# names cannot introduce declarations. Block comments may nest in Rust.
TOKEN = re.compile(
    r'//[^\n]*|/\*|(?:br|cr|r)(\#*)".*?"\1|(?:b|c)?"(?:\\.|[^"\\])*"|'
    r"(?:b)?'(?:\\.|[^'\\\n])'|(?:r#)?[A-Za-z_][A-Za-z_0-9]*|[^\s]",
    re.DOTALL,
)


def tokens(source):
    cursor = 0
    while match := TOKEN.search(source, cursor):
        cursor = match.end()
        token = match.group()
        if token.startswith("//"):
            continue
        if token == "/*":
            depth = 1
            while depth:
                boundary = re.search(r"/\*|\*/", source[cursor:])
                if boundary is None:
                    raise ValueError("unterminated block comment")
                depth += 1 if boundary.group() == "/*" else -1
                cursor += boundary.end()
            continue
        yield token


def declarations(source):
    items = list(tokens(source))
    found = set()
    for index, token in enumerate(items):
        if token != "extern":
            continue
        start = index + 1
        if start < len(items) and items[start].startswith('"'):
            start += 1
        if start >= len(items) or items[start] != "{":
            continue  # A definition, a function pointer, or extern crate.
        depth = 1
        nesting = 0
        expect_name = False
        declared = False
        for item in items[start + 1 :]:
            if item == "{":
                depth += 1
            elif item == "}":
                depth -= 1
                if depth == 0:
                    break
            if depth != 1:
                continue
            if item == "link_name":
                found.add("<link_name>")
            if item in ("(", "["):
                nesting += 1
            elif item in (")", "]"):
                nesting -= 1
            elif item == ";" and nesting == 0:
                declared = False
            elif expect_name:
                if item != "mut":
                    found.add(item.removeprefix("r#"))
                    expect_name = False
            elif nesting == 0 and not declared and item in ("fn", "static", "type"):
                expect_name = True
                declared = True
        else:
            raise ValueError("unterminated extern block")
    return found


def main():
    allowed = set()
    for line in (ROOT / "scripts/ffi-allowlist.txt").read_text().splitlines():
        fields = line.split("#", 1)[0].split()
        if not fields:
            continue
        if len(fields) != 2 or tuple(fields) in allowed:
            raise ValueError(f"invalid or duplicate allowlist entry: {line}")
        allowed.add(tuple(fields))
    actual = set()
    for source in sorted((ROOT / "crates").rglob("*.rs")):
        relative = source.relative_to(ROOT).as_posix()
        actual.update((relative, name) for name in declarations(source.read_text()))
    new = actual - allowed
    stale = allowed - actual
    if new:
        print("check-ffi: these foreign symbols are not allowed; use libc or objc2:")
        for source, name in sorted(new):
            print(f"  {source} {name}")
    if stale:
        print("check-ffi: remove these stale entries from scripts/ffi-allowlist.txt:")
        for source, name in sorted(stale):
            print(f"  {source} {name}")
    return int(bool(new or stale))


if __name__ == "__main__":
    raise SystemExit(main())
