#!/usr/bin/env python3
"""Check that the audio-path crates deny panicking lints at their library roots.

The audio thread must not panic, so product code in these crates reaches data
with `get`, iterators and slice patterns (ADR 0284). Clippy enforces that once
the attribute is in place; this check keeps the attribute in each root and
refuses a local `allow` or `expect` that would switch a lint off again.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# Crates whose library code runs on an audio thread: the DAW's callback, the
# plugin helper's processing thread, or the bundled plugin's.
AUDIO_CRATES = (
    "echode-time",
    "echode-realtime",
    "echode-dsp",
    "echode-sine-pad",
    "echode-sandbox",
    "echode-plugin-host",
    "echode-engine",
    "echode-audio-io",
)
LINTS = ("indexing_slicing", "unwrap_used", "expect_used", "panic")
REQUIRED = "#![cfg_attr(not(test),deny(" + ",".join(f"clippy::{lint}" for lint in LINTS) + "))]"
# Measurement code kept as it was measured, built only with a spike feature.
EXEMPT = {"crates/echode-engine/src/graph.rs"}
ESCAPE = re.compile(r"\b(?:allow|expect)\s*\(([^()]*)\)")


def problems(root):
    found = []
    for crate in AUDIO_CRATES:
        lib = root / "crates" / crate / "src" / "lib.rs"
        if not lib.exists():
            found.append(f"{lib.relative_to(root).as_posix()}: missing")
            continue
        if REQUIRED not in re.sub(r"\s+", "", lib.read_text()):
            found.append(f"{lib.relative_to(root).as_posix()}: does not deny {', '.join(LINTS)}")
        for source in sorted((root / "crates" / crate / "src").rglob("*.rs")):
            relative = source.relative_to(root).as_posix()
            if relative in EXEMPT:
                continue
            text = source.read_text()
            for match in ESCAPE.finditer(text):
                names = {name.strip() for name in match.group(1).split(",")}
                for lint in LINTS:
                    if f"clippy::{lint}" in names:
                        line = text.count("\n", 0, match.start()) + 1
                        found.append(f"{relative}:{line}: allows clippy::{lint}")
    return found


def main():
    found = problems(ROOT)
    if found:
        print("check-audio-lints: audio-path crates must deny panicking lints (ADR 0284):")
        for problem in found:
            print(f"  {problem}")
    return int(bool(found))


if __name__ == "__main__":
    sys.exit(main())
