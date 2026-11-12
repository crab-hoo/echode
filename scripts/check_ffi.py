#!/usr/bin/env python3
"""Check handwritten Rust foreign declarations against the allowlist and the macOS SDK.

Each allowed declaration is an exact source path and Rust item name (ADR 0265), with the
SDK header that declares it. The Rust signature is translated to C and compiled against
that header: every parameter, return, static and struct field must be the same C type,
each struct must have the SDK's layout, and the SDK must not mark the symbol deprecated
or newer than the deployment target (ENG-15, ADR 0326).
"""

import re
import subprocess
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# Keep strings as opaque tokens (including raw strings), so examples and ABI
# names cannot introduce declarations. Block comments may nest in Rust. A number
# with its suffix is one token, so array lengths can be read.
TOKEN = re.compile(
    r'//[^\n]*|/\*|(?:br|cr|r)(\#*)".*?"\1|(?:b|c)?"(?:\\.|[^"\\])*"|'
    r"(?:b)?'(?:\\.|[^'\\\n])'|(?:r#)?[A-Za-z_][A-Za-z_0-9]*|[0-9][A-Za-z_0-9]*|[^\s]",
    re.DOTALL,
)
IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
HEADER = re.compile(r"[A-Za-z0-9_]+(?:/[A-Za-z0-9_]+)*\.h")
OPENING = {"(": ")", "[": "]", "{": "}"}
CLOSING = set(OPENING.values())
# C spellings of the Rust types a declaration may use. `std::ffi`, `core::ffi`, `std::os::raw`
# and `libc` name the same C types. Fixed-width integers are their <stdint.h> names, so `u64`
# for a `size_t` parameter fails: the ABI matches, but the header says another type.
PRIMITIVES = {
    "c_void": "void",
    "c_char": "char",
    "c_schar": "signed char",
    "c_uchar": "unsigned char",
    "c_short": "short",
    "c_ushort": "unsigned short",
    "c_int": "int",
    "c_uint": "unsigned int",
    "c_long": "long",
    "c_ulong": "unsigned long",
    "c_longlong": "long long",
    "c_ulonglong": "unsigned long long",
    "c_float": "float",
    "c_double": "double",
    "i8": "int8_t",
    "u8": "uint8_t",
    "i16": "int16_t",
    "u16": "uint16_t",
    "i32": "int32_t",
    "u32": "uint32_t",
    "i64": "int64_t",
    "u64": "uint64_t",
    "isize": "intptr_t",
    "usize": "size_t",
    "f32": "float",
    "f64": "double",
    "bool": "_Bool",
}
PATH_PREFIXES = {("std", "ffi"), ("core", "ffi"), ("std", "os", "raw"), ("libc",)}


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


def extern_blocks(items):
    """The tokens inside each `extern [ABI] { ... }` block."""
    for index, token in enumerate(items):
        if token != "extern":
            continue
        start = index + 1
        if start < len(items) and items[start].startswith('"'):
            start += 1
        if start >= len(items) or items[start] != "{":
            continue  # A definition, a function pointer, or extern crate.
        depth = 1
        for end in range(start + 1, len(items)):
            if items[end] == "{":
                depth += 1
            elif items[end] == "}":
                depth -= 1
                if depth == 0:
                    yield items[start + 1 : end]
                    break
        else:
            raise ValueError("unterminated extern block")


def split_top_level(items, separator):
    """Split at `separator` outside brackets. `<`/`>` nest too, except the `>` of `->`."""
    parts, current, nesting, angles = [], [], 0, 0
    for index, token in enumerate(items):
        if token in OPENING:
            nesting += 1
        elif token in CLOSING:
            nesting -= 1
        elif token == "<":
            angles += 1
        elif token == ">" and angles and (index == 0 or items[index - 1] != "-"):
            angles -= 1
        if token == separator and nesting == 0 and angles == 0:
            parts.append(current)
            current = []
        else:
            current.append(token)
    parts.append(current)
    return parts


def foreign_items(source):
    """(kind, name, tokens after the name) for each item in each extern block."""
    found = []
    for block in extern_blocks(list(tokens(source))):
        *items, rest = split_top_level(block, ";")
        if rest:
            raise ValueError("unterminated foreign item")
        for item in items:
            if "link_name" in item:
                found.append(("link_name", "<link_name>", []))
            nesting = 0
            for index, token in enumerate(item):
                if token in OPENING:
                    nesting += 1
                elif token in CLOSING:
                    nesting -= 1
                elif nesting == 0 and token in ("fn", "static", "type"):
                    position = index + 1
                    if token == "static" and item[position : position + 1] == ["mut"]:
                        position += 1
                    if position < len(item):
                        found.append((token, item[position].removeprefix("r#"), item[position + 1 :]))
                    break
    return found


def declarations(source):
    return {name for _, name, _ in foreign_items(source)}


class Unsupported(Exception):
    """A Rust type or item the C translation does not cover. The check fails closed."""


@dataclass(frozen=True)
class Base:
    spelling: str


@dataclass(frozen=True)
class StructType:
    name: str


@dataclass(frozen=True)
class Pointer:
    pointee: object
    const: bool


@dataclass(frozen=True)
class Array:
    element: object
    length: int


@dataclass(frozen=True)
class Struct:
    name: str
    repr_c: bool
    fields: list  # [(name, type tokens)]; empty for an opaque struct


def parse_type(items, structs):
    kind, position = parse_type_at(items, 0, structs)
    if position != len(items):
        raise Unsupported(f"unsupported type `{' '.join(items)}`")
    return kind


def parse_type_at(items, position, structs):
    text = " ".join(items)
    if position >= len(items):
        raise Unsupported(f"unsupported type `{text}`")
    token = items[position]
    if token == "*":
        qualifier = items[position + 1] if position + 1 < len(items) else ""
        if qualifier not in ("mut", "const"):
            raise Unsupported(f"unsupported type `{text}`")
        pointee, position = parse_type_at(items, position + 2, structs)
        return Pointer(pointee, qualifier == "const"), position
    if token == "[":
        element, position = parse_type_at(items, position + 1, structs)
        length = items[position + 1] if position + 1 < len(items) else ""
        if items[position : position + 1] != [";"] or items[position + 2 : position + 3] != ["]"]:
            raise Unsupported(f"unsupported type `{text}`")
        literal = re.fullmatch(r"([0-9][0-9_]*)(?:usize)?", length)
        if literal is None:
            raise Unsupported(f"unsupported array length `{length}` in `{text}`: use an integer literal")
        return Array(element, int(literal[1].replace("_", ""))), position + 3
    if not IDENTIFIER.fullmatch(token):
        raise Unsupported(f"unsupported type `{text}`")
    segments = [token]
    position += 1
    while items[position : position + 2] == [":", ":"] and position + 2 < len(items):
        segments.append(items[position + 2])
        position += 3
    prefix, name = tuple(segments[:-1]), segments[-1]
    if items[position : position + 1] == ["<"]:
        raise Unsupported(f"unsupported type `{text}`")
    if name in PRIMITIVES and (not prefix or prefix in PATH_PREFIXES):
        return Base(PRIMITIVES[name]), position
    if not prefix and name in structs:
        return StructType(name), position
    raise Unsupported(f"unsupported type `{'::'.join(segments)}`: use a C type or a #[repr(C)] struct in this file")


def struct_definitions(items):
    """`struct NAME { ... }` definitions in a file, with whether exactly `#[repr(C)]` precedes them."""
    found = {}
    for index, token in enumerate(items):
        if token != "struct" or index + 2 >= len(items) or items[index + 2] != "{":
            continue
        name = items[index + 1]
        if name in found:
            raise Unsupported(f"struct {name} is defined more than once")
        position = index - 1
        if position >= 0 and items[position] == ")":
            while position >= 0 and items[position] != "(":
                position -= 1
            position -= 1
        if position >= 0 and items[position] == "pub":
            position -= 1
        repr_c = False
        while position >= 1 and items[position] == "]":
            end = position
            depth = 0
            while position >= 0:
                depth += {"]": 1, "[": -1}.get(items[position], 0)
                if depth == 0:
                    break
                position -= 1
            attribute = items[position + 1 : end]
            if attribute and attribute[0] == "repr":
                repr_c = attribute == ["repr", "(", "C", ")"]
            position -= 2  # The `[` and its `#`.
        depth = 1
        for end in range(index + 3, len(items)):
            depth += {"{": 1, "}": -1}.get(items[end], 0)
            if depth == 0:
                break
        else:
            raise ValueError(f"unterminated struct {name}")
        fields = []
        for field in split_top_level(items[index + 3 : end], ","):
            while field[:1] == ["#"]:
                close = field.index("]")
                field = field[close + 1 :]
            if field[:1] == ["pub"]:
                field = field[1:]
                if field[:1] == ["("]:
                    field = field[field.index(")") + 1 :]
            if not field:
                continue
            if len(field) < 3 or field[1] != ":":
                raise Unsupported(f"unsupported field in struct {name}")
            fields.append((field[0].removeprefix("r#"), field[2:]))
        opaque = all(field_name.startswith("_") for field_name, _ in fields)
        found[name] = Struct(name, repr_c, [] if opaque else fields)
    return found


def parse_function(name, rest, structs):
    if rest[:1] != ["("]:
        raise Unsupported(f"{name}: unsupported generic or malformed function")
    depth = 0
    for close, token in enumerate(rest):
        depth += 1 if token in OPENING else -1 if token in CLOSING else 0
        if depth == 0:
            break
    parameters = []
    inside = rest[1:close]
    for parameter in split_top_level(inside, ",") if inside else []:
        if not parameter:
            continue
        if "." in parameter:
            raise Unsupported(f"{name}: unsupported variadic function")
        pattern = parameter[: parameter.index(":")] if ":" in parameter else []
        if pattern not in ([pattern[-1]] if pattern else [], ["mut", pattern[-1]] if pattern else []):
            raise Unsupported(f"{name}: unsupported parameter `{' '.join(parameter)}`")
        if not pattern or not IDENTIFIER.fullmatch(pattern[-1].removeprefix("r#")):
            raise Unsupported(f"{name}: unsupported parameter `{' '.join(parameter)}`")
        parameters.append(value_type(parse_type(parameter[len(pattern) + 1 :], structs), name))
    after = rest[close + 1 :]
    if not after:
        return Base("void"), parameters
    if after[:2] != ["-", ">"]:
        raise Unsupported(f"{name}: unsupported return `{' '.join(after)}`")
    return value_type(parse_type(after[2:], structs), name), parameters


def value_type(kind, name):
    """A parameter, return or static cannot be `c_void` or an array by value."""
    if kind == Base("void") or isinstance(kind, Array):
        raise Unsupported(f"{name}: unsupported by-value `void` or array")
    return kind


def c_declarator(kind, declarator="", qualified=False):
    """The C spelling of `kind`, declaring `declarator` (empty for an abstract type)."""
    const = " const" if qualified else ""
    if isinstance(kind, Base):
        return f"{kind.spelling}{const} {declarator}".rstrip()
    if isinstance(kind, StructType):
        return f"struct {kind.name}{const} {declarator}".rstrip()
    if isinstance(kind, Pointer):
        if isinstance(kind.pointee, Array):
            raise Unsupported("unsupported pointer to an array")
        return c_declarator(kind.pointee, f"*{const} {declarator}".rstrip(), kind.const)
    length = f"[{kind.length}]"
    if declarator.startswith("*"):
        return c_declarator(kind.element, f"({declarator}){length}", qualified)
    return c_declarator(kind.element, f"{declarator}{length}", qualified)


def referenced_structs(kinds, structs):
    """Struct names reachable from `kinds`, including through struct fields, in a stable order."""
    order = []
    pending = list(kinds)
    while pending:
        kind = pending.pop(0)
        if isinstance(kind, Pointer):
            pending.append(kind.pointee)
        elif isinstance(kind, Array):
            pending.append(kind.element)
        elif isinstance(kind, StructType) and kind.name not in order:
            order.append(kind.name)
            pending.extend(parse_type(field, structs) for _, field in structs[kind.name].fields)
    return order


def c_string(text):
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"') + '"'


class Unit:
    """One C translation unit: an SDK header and the checks that need it, one per line.

    Clang reports a deprecated symbol, or one newer than the deployment target, where
    `__typeof__(&symbol)` names it, so the type checks also carry those diagnostics.
    """

    def __init__(self, header):
        self.lines = ["#include <stddef.h>", "#include <stdint.h>", f"#include <{header}>"]
        self.descriptions = {}

    def add(self, line, description=None):
        self.lines.append(line)
        if description is not None:
            self.descriptions[len(self.lines)] = description

    def check(self, condition, description):
        self.add(f"_Static_assert({condition}, {c_string(description)});", description)

    def text(self):
        return "\n".join(self.lines) + "\n"


def add_struct(unit, source, struct, structs):
    label = f"{source} struct {struct.name}"
    if not struct.repr_c:
        raise Unsupported(f"struct {struct.name} needs exactly #[repr(C)]")
    if not struct.fields:
        return  # Opaque: only used behind a pointer, so only the tag is compared.
    mirror = f"echode_rust_{struct.name}"
    fields = [(name, parse_type(field, structs)) for name, field in struct.fields]
    members = " ".join(f"{c_declarator(kind, name)};" for name, kind in fields)
    unit.add(f"struct {mirror} {{ {members} }};")
    unit.check(f"sizeof(struct {struct.name}) == sizeof(struct {mirror})", f"{label}: size")
    unit.check(f"_Alignof(struct {struct.name}) == _Alignof(struct {mirror})", f"{label}: alignment")
    for name, kind in fields:
        unit.check(
            f"offsetof(struct {struct.name}, {name}) == offsetof(struct {mirror}, {name})",
            f"{label}: offset of {name}",
        )
        unit.check(
            f"__builtin_types_compatible_p(__typeof__(((struct {struct.name} *)0)->{name}), {c_declarator(kind)})",
            f"{label}: type of {name}",
        )


def build_unit(source, header, names, text):
    """The C checks for `names` from `source`, or the reasons the translation failed."""
    items = list(tokens(text))
    declared = {}
    for kind, name, rest in foreign_items(text):
        declared.setdefault(name, []).append((kind, rest))
    problems = []
    unit = Unit(header)
    try:
        structs = struct_definitions(items)
    except Unsupported as error:
        return None, [f"{source}: {error}"]
    checks = []
    used = []
    for name in sorted(names):
        label = f"{source} {name}"
        try:
            (kind, rest), *others = declared[name]
            if others:
                raise Unsupported("declared more than once")
            if kind == "fn":
                result, parameters = parse_function(name, rest, structs)
                arguments = ", ".join(c_declarator(parameter) for parameter in parameters) or "void"
                expected = c_declarator(result, f"(*)({arguments})")
                checks.append((f"__builtin_types_compatible_p(__typeof__(&{name}), {expected})", label))
                used.extend([result, *parameters])
            elif kind == "static":
                if rest[:1] != [":"]:
                    raise Unsupported("malformed static")
                static = value_type(parse_type(rest[1:], structs), name)
                checks.append((f"__builtin_types_compatible_p(__typeof__({name}), {c_declarator(static)})", label))
                used.append(static)
            else:
                raise Unsupported(f"unsupported foreign {kind}")
        except Unsupported as error:
            problems.append(f"{label}: {error}")
    try:
        for struct in referenced_structs(used, structs):
            add_struct(unit, source, structs[struct], structs)
    except Unsupported as error:
        problems.append(f"{source}: {error}")
    for condition, label in checks:
        unit.check(condition, label)
    return unit, problems


def deployment_target():
    text = (ROOT / "scripts/build-platform.env").read_text()
    match = re.search(r"^export MACOSX_DEPLOYMENT_TARGET=([0-9]+(?:\.[0-9]+)*)$", text, re.MULTILINE)
    if match is None:
        raise ValueError("scripts/build-platform.env: missing MACOSX_DEPLOYMENT_TARGET")
    return match[1]


def compile_unit(unit, target):
    """Compiler errors for `unit`, each with the check it belongs to when it is a generated line."""
    command = [
        "xcrun",
        "--sdk",
        "macosx",
        "clang",
        "-x",
        "c",
        "-std=c11",
        "-fsyntax-only",
        "-arch",
        "arm64",
        f"-mmacosx-version-min={target}",
        "-Werror",
        "-Wall",
        "-Wextra",
        "-Werror=deprecated-declarations",
        "-Werror=unguarded-availability",
        "-Werror=unguarded-availability-new",
        "-",
    ]
    try:
        result = subprocess.run(command, input=unit.text(), capture_output=True, text=True, check=False)
    except FileNotFoundError:
        return ["xcrun is missing: install the Xcode command line tools (make doctor)"]
    if result.returncode == 0:
        return []
    errors = []
    for line in result.stderr.splitlines():
        match = re.match(r"<stdin>:([0-9]+):[0-9]+: error: (.*)", line)
        if match:
            where = unit.descriptions.get(int(match[1]), f"generated line {match[1]}")
            errors.append(f"{where}: {match[2].removesuffix(': ' + where)}")
        elif ": error: " in line:
            errors.append(line)
    return errors or [result.stderr.strip() or f"clang exited with {result.returncode}"]


def compare_with_sdk(allowed):
    """Problems for each allowed declaration, grouped by source and header."""
    target = deployment_target()
    groups = {}
    for source, name, header in allowed:
        groups.setdefault((source, header), set()).add(name)
    problems = []
    for (source, header), names in sorted(groups.items()):
        unit, found = build_unit(source, header, names, (ROOT / source).read_text())
        problems.extend(found)
        if unit is not None and unit.descriptions:
            problems.extend(f"{header}: {error}" for error in compile_unit(unit, target))
    return target, problems


def read_allowlist():
    allowed = set()
    names = set()
    for line in (ROOT / "scripts/ffi-allowlist.txt").read_text().splitlines():
        fields = line.split("#", 1)[0].split()
        if not fields:
            continue
        if (
            len(fields) != 3
            or tuple(fields[:2]) in names
            or not IDENTIFIER.fullmatch(fields[1])
            or not HEADER.fullmatch(fields[2])
        ):
            print(f"check-ffi: invalid or duplicate allowlist entry (path, symbol, SDK header): {line}")
            return None
        names.add(tuple(fields[:2]))
        allowed.add(tuple(fields))
    return allowed


def main():
    allowed = read_allowlist()
    if allowed is None:
        return 1
    actual = set()
    for source in sorted((ROOT / "crates").rglob("*.rs")):
        relative = source.relative_to(ROOT).as_posix()
        actual.update((relative, name) for name in declarations(source.read_text()))
    listed = {(source, name) for source, name, _ in allowed}
    new = actual - listed
    stale = listed - actual
    if new:
        print("check-ffi: these foreign symbols are not allowed; use libc, mach2 or objc2:")
        for source, name in sorted(new):
            print(f"  {source} {name}")
    if stale:
        print("check-ffi: remove these stale entries from scripts/ffi-allowlist.txt:")
        for source, name in sorted(stale):
            print(f"  {source} {name}")
    if new or stale:
        return 1
    target, problems = compare_with_sdk(allowed)
    if problems:
        print(f"check-ffi: these declarations differ from the macOS SDK (deployment target {target}):")
        for problem in problems:
            print(f"  {problem}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
