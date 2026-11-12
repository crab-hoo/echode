#!/usr/bin/env python3
"""Check that every workspace crate inherits the workspace lints (ADR 0321).

`Cargo.toml` forbids an `unsafe` block or impl without a `// SAFETY:` comment,
and `forbid` makes rustc refuse a local `allow` or `expect` of that lint. A crate
that leaves out `[lints] workspace = true` escapes it, and every other workspace
lint, without any message. This check refuses that, and a workspace manifest
that weakens either level. The members come from `[workspace] members`, as in
`check_typed_errors.py` (ADR 0320), so a new crate is checked from its first commit.
"""

import sys
import tomllib
from pathlib import Path

from check_typed_errors import members

ROOT = Path(__file__).resolve().parent.parent
# Lint name, its table and the levels this repository accepts for it.
REQUIRED = (
    ("clippy", "undocumented_unsafe_blocks", {"forbid"}),
    ("rust", "unsafe_op_in_unsafe_fn", {"deny", "forbid"}),
)


def level(setting):
    """A lint's level, whether written as a string or as `{ level = ... }`."""
    return setting.get("level") if isinstance(setting, dict) else setting


def problems(root):
    directories, found = members(root)
    if not directories:
        return found
    lints = tomllib.loads((root / "Cargo.toml").read_text()).get("workspace", {}).get("lints", {})
    for table, name, accepted in REQUIRED:
        setting = level(lints.get(table, {}).get(name))
        if setting not in accepted:
            wanted = " or ".join(sorted(accepted))
            found.append(f"Cargo.toml: {table}::{name} is {setting or 'unset'}, not {wanted}")
    for directory in directories:
        manifest = directory / "Cargo.toml"
        relative = manifest.relative_to(root).as_posix()
        try:
            inherited = tomllib.loads(manifest.read_text()).get("lints", {}).get("workspace")
        except (OSError, tomllib.TOMLDecodeError) as error:
            found.append(f"{relative}: unreadable ({error})")
            continue
        if inherited is not True:
            found.append(f"{relative}: does not inherit the workspace lints")
    return found


def main():
    found = problems(ROOT)
    if found:
        print("check-workspace-lints: every crate keeps the workspace lints (ADR 0321):")
        for problem in found:
            print(f"  {problem}")
    return int(bool(found))


if __name__ == "__main__":
    sys.exit(main())
