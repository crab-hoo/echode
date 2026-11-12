#!/usr/bin/env python3
"""Measure injected failure latency through verify.py, never through product/device tests."""

import argparse
import io
import json
import subprocess
import tempfile
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest.mock import patch

import verify

LEGACY_ORDER = [
    "check-verification",
    "check",
    "check-format",
    "check-product-contracts",
    "rtsan",
    "check-input",
    "check-bridge-features",
    "check-bridge-asan",
    "check-tsan-runtime",
    "check-bridge-tsan",
    "check-bridge-miri",
]


def measure(policy, scenario, reordered, delay):
    # Only synthetic recipe costs/failures are substituted. Selection, ordering,
    # preflight, process execution, first-failure recording and reporting are real.
    with tempfile.TemporaryDirectory(prefix="echode-verification-latency-") as folder:
        root = Path(folder)
        (root / "scripts").mkdir()
        (root / "source").write_text("fixture")
        (root / ".gitignore").write_text("/target\n")
        originals = {c["id"]: c for c in policy["checks"] if c["mode"] == "automatic"}
        if set(originals) != set(LEGACY_ORDER):
            raise ValueError("update the comparison fixture for the current automatic check set")
        checks, recipes = [], []
        for name in LEGACY_ORDER:
            original = originals[name]
            check = {
                "id": name,
                "mode": "automatic",
                "areas": [],
                "inputs": ["source"],
                "timeout": 10,
                "purpose": "injected failure latency fixture",
            }
            if reordered:
                check.update({key: original[key] for key in ("priority", "after") if key in original})
                if name == "check-tsan-runtime":
                    check["preflight"] = ["runtime"]
            checks.append(check)
            failed = name == scenario
            recipes += [
                f"{name}:",
                f"\t@python3 -c 'import time; time.sleep({delay}); raise SystemExit({int(failed)})'",
            ]
        preflight = {
            "runtime": {
                "command": ["python3", "-c", f"raise SystemExit({int(scenario == 'check-tsan-runtime')})"],
                "timeout": 5,
                "remedy": "prepare fixture runtime",
            }
        }
        fixture = {
            "version": 1,
            "global_inputs": [],
            "infrastructure": [],
            "areas": {},
            "preflights": preflight,
            "checks": checks,
        }
        (root / verify.POLICY).write_text(json.dumps(fixture))
        (root / "Makefile").write_text("\n".join(recipes) + "\n")
        subprocess.run(["git", "init", "--quiet", str(root)], check=True)
        subprocess.run(["git", "-C", str(root), "add", "."], check=True)
        options = argparse.Namespace(
            base=None,
            all=True,
            full=False,
            only=[],
            run=[],
            defer=[],
            reason=None,
            record_only=False,
            plan=False,
            force=False,
        )
        with (
            patch.object(verify, "tool_identity", return_value={"fixture": "same-tools"}),
            redirect_stdout(io.StringIO()),
            redirect_stderr(io.StringIO()),
        ):
            code = verify.verify(root, options)
        report = json.loads((root / "target/verification/report.json").read_text())
        if code != 1 or report["execution"]["first_failure"]["id"] not in (scenario, "runtime"):
            raise ValueError("injected failure was not detected at the intended boundary")
        return {
            "selected": list(report["selected"]),
            "execution": report["execution"],
            "results": {name: result["status"] for name, result in report["results"].items()},
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--delay", type=float, default=0.2)
    parser.add_argument("--output", type=Path, default=Path("target/verification/failure-latency.json"))
    args = parser.parse_args()
    if not 0 <= args.delay <= 5:
        parser.error("delay must be between 0 and 5 seconds")
    policy = verify.load_policy(verify.ROOT)
    data = {
        "scope": "synthetic failure latency, not product runtime or historical session reproduction",
        "delay_per_check": args.delay,
        "scenarios": {},
    }
    for scenario in ("check-format", "check-verification", "check-bridge-asan", "check-tsan-runtime"):
        baseline = measure(policy, scenario, False, args.delay)
        current = measure(policy, scenario, True, args.delay)
        if set(baseline["selected"]) != set(current["selected"]):
            raise ValueError("comparison changed the selected check set")
        data["scenarios"][scenario] = {"baseline": baseline, "current": current}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(data, indent=2) + "\n")


if __name__ == "__main__":
    main()
