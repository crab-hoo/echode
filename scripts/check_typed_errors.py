#!/usr/bin/env python3
"""Check that crates with typed errors do not return `String` errors again.

A crate on this list has an error enum at its boundary (M1, ADR 0291), so its
callers branch on the cause instead of on text. Product code in it must not
name `Result<_, String>` anywhere: not in a signature, a channel or a closure.
Nor may a `map_err` turn the error it receives into text (ADR 0294): a variant
that holds `error.to_string()` hides the cause the same way. Tests may, since
a fixture's message is not a boundary. Text is still made where a report
reaches the app; that is a `String` field, not a `Result`.
A file in TYPED_FILES has the same checks while its crate is being migrated.
History and partially migrated files also reject untyped PackageError messages.
"""

from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parent.parent
# Crates whose boundary errors are enums. Add a crate when its pass is done.
TYPED_CRATES = (
    'echode-audio-io',
    'echode-document',
    'echode-engine',
    'echode-ffi',
    'echode-history',
    'echode-model',
    'echode-plugin-host',
    'echode-sandbox',
)
# A large crate may migrate one storage boundary at a time (ADR 0296).
TYPED_FILES = ()
PACKAGE_MESSAGE = re.compile(r'\bPackageError\s*::\s*(?:new|Message)\s*\(')
STRING = {'String', 'std::string::String', 'alloc::string::String'}
RESULT = re.compile(r'\bResult\s*<')
MAP_ERR = re.compile(r'\.map_err\s*\(')
TEXT = re.compile(r'\.to_string\s*\(\s*\)|\bformat!\s*\(')
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


def is_test(path):
    return path.name.endswith('_tests.rs') or 'tests' in path.parts


def problems(root):
    found = []
    sources = []
    for crate in TYPED_CRATES:
        source = root / 'crates' / crate / 'src'
        if not source.is_dir():
            found.append(f'crates/{crate}/src: missing')
            continue
        sources.extend(sorted(source.rglob('*.rs')))
    for relative in TYPED_FILES:
        source = root / relative
        if not source.is_file():
            found.append(f'{relative}: missing')
            continue
        sources.append(source)
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
        if relative.parts[1] == 'echode-history' or relative.as_posix() in TYPED_FILES:
            code = COMMENT.sub(lambda match: ' ' * len(match.group(0)), text)
            for match in PACKAGE_MESSAGE.finditer(code):
                line = code.count('\n', 0, match.start()) + 1
                found.append(f'{relative.as_posix()}:{line}: returns an untyped package message')
    return found


def main():
    found = problems(ROOT)
    if found:
        print('check-typed-errors: these boundaries return error enums (ADR 0291, 0296):')
        for problem in found:
            print(f'  {problem}')
    return int(bool(found))


if __name__ == '__main__':
    sys.exit(main())
