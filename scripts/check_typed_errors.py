#!/usr/bin/env python3
"""Check that crates with typed errors do not return `String` errors again.

A crate on this list has an error enum at its boundary (M1, ADR 0291), so its
callers branch on the cause instead of on text. Product code in it must not
name `Result<_, String>` anywhere: not in a signature, a channel or a closure.
Tests may, since a fixture's message is not a boundary. Text is still made
where a report reaches the app; that is a `String` field, not a `Result`.
"""

from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parent.parent
# Crates whose boundary errors are enums. Add a crate when its pass is done.
TYPED_CRATES = (
    'echode-audio-io',
    'echode-document',
    'echode-sandbox',
)
STRING = {'String', 'std::string::String', 'alloc::string::String'}
RESULT = re.compile(r'\bResult\s*<')
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


def is_test(path):
    return path.name.endswith('_tests.rs') or 'tests' in path.parts


def problems(root):
    found = []
    for crate in TYPED_CRATES:
        source = root / 'crates' / crate / 'src'
        if not source.is_dir():
            found.append(f'crates/{crate}/src: missing')
            continue
        for path in sorted(source.rglob('*.rs')):
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
    return found


def main():
    found = problems(ROOT)
    if found:
        print('check-typed-errors: these crates return error enums (ADR 0291):')
        for problem in found:
            print(f'  {problem}')
    return int(bool(found))


if __name__ == '__main__':
    sys.exit(main())
