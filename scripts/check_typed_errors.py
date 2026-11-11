#!/usr/bin/env python3
"""Check that no workspace crate returns `String` errors again.

Every crate's boundary errors are enums (M1, ADR 0291 to 0320), so callers
branch on the cause instead of on text. Product code must not name
`Result<_, String>` anywhere: not in a signature, a channel or a closure.
Nor may a `map_err` turn the error it receives into text (ADR 0294): a variant
that holds `error.to_string()` hides the cause the same way. Tests may, since
a fixture's message is not a boundary. Text is still made where a report
reaches the app; that is a `String` field, not a `Result`.
I/O error constructors must not take formatted text either, even outside a
`map_err` (ADR 0311, 0320): an inner error type keeps the name or OS cause.
An I/O error does not carry a string literal either, unless its kind is the
whole cause (ADR 0317). A literal under InvalidData folds every field and
value into one sentence. A `&'static str` held in a variable is not found:
this is syntax, not type inference.
The crates are the workspace members in Cargo.toml (ADR 0320), so a new crate
is checked from the commit that adds it, without a list to update here.
The history crate also rejects untyped PackageError messages.
"""

from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parent.parent
HISTORY = ('crates', 'echode-history')
# Kinds that are the whole cause, so a literal there is only a remark.
KIND_IS_CAUSE = {'TimedOut', 'UnexpectedEof', 'WriteZero'}
LITERAL = re.compile(r'\s*r?#*"')
PACKAGE_MESSAGE = re.compile(r'\bPackageError\s*::\s*(?:new|Message)\s*\(')
STRING = {'String', 'std::string::String', 'alloc::string::String'}
RESULT = re.compile(r'\bResult\s*<')
MAP_ERR = re.compile(r'\.map_err\s*\(')
TEXT = re.compile(r'\.to_string\s*\(\s*\)|\bformat!\s*\(')
IO_ERROR = re.compile(r'\b(?:std\s*::\s*)?io\s*::\s*Error\s*::\s*(other|new)\s*\(')
COMMENT = re.compile(r'//[^\n]*')
OPEN = '<([{'
CLOSE = '>)]}'


def arguments(text, start):
    """The top-level type arguments of the `<` at `start`, or None if unclosed."""
    depth = 0
    pieces = []
    current = []
    index = start
    while index < len(text):
        char = text[index]
        if char == '-' and text.startswith('->', index):
            current.append('->')
            index += 2
            continue
        if char in OPEN:
            depth += 1
            if depth > 1:
                current.append(char)
        elif char in CLOSE:
            depth -= 1
            if depth == 0:
                pieces.append(''.join(current).strip())
                # A trailing comma leaves an empty last argument.
                return [piece for piece in pieces if piece]
            current.append(char)
        elif char == ',' and depth == 1:
            pieces.append(''.join(current).strip())
            current = []
        else:
            current.append(char)
        index += 1
    return None


def string_results(text):
    """Line numbers where a `Result` takes `String` as its error type."""
    code = COMMENT.sub(lambda match: ' ' * len(match.group(0)), text)
    lines = []
    for match in RESULT.finditer(code):
        found = arguments(code, match.end() - 1)
        if found and len(found) == 2 and re.sub(r'\s+', '', found[1]) in STRING:
            lines.append(code.count('\n', 0, match.start()) + 1)
    return lines


def closing(text, start):
    """The index after the bracket that closes the one at `start`, or None."""
    depth = 0
    for index in range(start, len(text)):
        if text[index] in OPEN:
            depth += 1
        elif text[index] in CLOSE:
            depth -= 1
            if depth == 0:
                return index + 1
    return None


def text_errors(text):
    """Line numbers where a `map_err` makes text from the error it receives."""
    code = COMMENT.sub(lambda match: ' ' * len(match.group(0)), text)
    lines = []
    for match in MAP_ERR.finditer(code):
        end = closing(code, match.end() - 1)
        if end is not None and TEXT.search(code, match.end(), end):
            lines.append(code.count('\n', 0, match.start()) + 1)
    return lines


def text_io_errors(text):
    """Line numbers where an I/O error wraps text rather than a typed cause."""
    code = COMMENT.sub(lambda match: ' ' * len(match.group(0)), text)
    lines = []
    for match in IO_ERROR.finditer(code):
        end = closing(code, match.end() - 1)
        if end is None:
            continue
        # Formatted text folds a name, a value or an OS cause into a sentence;
        # since ADR 0320 no crate needs it, so an inner error type holds them.
        # This is syntax, not type inference.
        if TEXT.search(code, match.end(), end):
            lines.append(code.count('\n', 0, match.start()) + 1)
    return lines


def top_level(body):
    """The comma-separated arguments of a call's body, outside nested brackets."""
    depth = 0
    pieces = ['']
    for char in body:
        if char in OPEN:
            depth += 1
        elif char in CLOSE:
            depth -= 1
        if char == ',' and depth == 0:
            pieces.append('')
        else:
            pieces[-1] += char
    return pieces


def constant_io_errors(text):
    """Line numbers where an I/O error carries only a string literal."""
    code = COMMENT.sub(lambda match: ' ' * len(match.group(0)), text)
    lines = []
    for match in IO_ERROR.finditer(code):
        end = closing(code, match.end() - 1)
        if end is None:
            continue
        pieces = top_level(code[match.end():end - 1])
        if match.group(1) == 'other':
            literal = LITERAL.match(pieces[0])
        else:
            kind = re.split(r'::', pieces[0])[-1].strip()
            literal = len(pieces) > 1 and LITERAL.match(pieces[1]) and kind not in KIND_IS_CAUSE
        if literal:
            lines.append(code.count('\n', 0, match.start()) + 1)
    return lines


def is_test(path):
    return path.name.endswith('_tests.rs') or 'tests' in path.parts


def members(root):
    """Each workspace member's directory, and problems with the manifest."""
    manifest = root / 'Cargo.toml'
    if not manifest.is_file():
        return [], ['Cargo.toml: missing']
    try:
        listed = tomllib.loads(manifest.read_text())['workspace']['members']
    except (tomllib.TOMLDecodeError, KeyError, TypeError) as error:
        return [], [f'Cargo.toml: no workspace members ({error})']
    found = []
    problems = []
    for member in listed:
        matched = sorted(root.glob(member)) if any(c in member for c in '*?[') else [root / member]
        if not matched:
            problems.append(f'{member}: missing')
        found.extend(matched)
    if not listed:
        problems.append('Cargo.toml: no workspace members')
    return found, problems


def problems(root):
    directories, found = members(root)
    sources = []
    for directory in directories:
        source = directory / 'src'
        if not source.is_dir():
            found.append(f'{source.relative_to(root).as_posix()}: missing')
            continue
        sources.extend(sorted(source.rglob('*.rs')))
    for path in sources:
        relative = path.relative_to(root)
        if is_test(relative):
            continue
        text = path.read_text()
        # An inline test module is a fixture too; product code precedes it.
        inline = re.search(r'#\[cfg\(test\)\]\s*mod\s+tests\s*\{', text)
        if inline:
            text = text[:inline.start()]
        for line in string_results(text):
            found.append(f'{relative.as_posix()}:{line}: returns a String error')
        for line in text_errors(text):
            found.append(f'{relative.as_posix()}:{line}: turns an error into text')
        for line in text_io_errors(text):
            found.append(f'{relative.as_posix()}:{line}: wraps error text in an I/O error')
        for line in constant_io_errors(text):
            found.append(f'{relative.as_posix()}:{line}: gives an I/O error only constant text')
        if relative.parts[:2] == HISTORY:
            code = COMMENT.sub(lambda match: ' ' * len(match.group(0)), text)
            for match in PACKAGE_MESSAGE.finditer(code):
                line = code.count('\n', 0, match.start()) + 1
                found.append(f'{relative.as_posix()}:{line}: returns an untyped package message')
    return found


def main():
    found = problems(ROOT)
    if found:
        print('check-typed-errors: workspace boundaries return error enums (ADR 0291, 0320):')
        for problem in found:
            print(f'  {problem}')
    return int(bool(found))


if __name__ == '__main__':
    sys.exit(main())
