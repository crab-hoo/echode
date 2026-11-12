#!/usr/bin/env python3
"""Diagnostic timings only; never create or reuse verification evidence."""

import argparse
import json
import os
import re
import shutil
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def resource_snapshot(root):
    # Diagnostic context only; machine load must never expire a successful check.
    return {
        "cpu_count": os.cpu_count(),
        "load_average": list(os.getloadavg()),
        "free_disk_bytes": shutil.disk_usage(root).free,
    }


def inventory(policy):
    contracts = policy.get("product_contracts", [])
    return {
        runner: {
            "contracts": len(cases),
            "mutations": sum(len(c.get("negative_controls") or [c.get("negative_control")]) for c in cases),
        }
        for runner in sorted({c["runner"] for c in contracts})
        for cases in [[c for c in contracts if c["runner"] == runner]]
    }


def contract_summary(directory):
    path = directory / "product-contracts.json"
    if not path.is_file():
        return None
    report = json.loads(path.read_text())
    return {"reused": report.get("reused", 0), "execution": report.get("execution", {})}


def record_execution(directory, report):
    """Called once at exit, under the verifier's lock. History is not a pass cache."""
    execution = dict(report.get("execution", {}))
    execution["checks"] = {
        name: {
            key: result[key]
            for key in ("status", "seconds", "timeout", "fingerprint", "previous_seconds", "previous_fingerprint")
            if key in result
        }
        for name, result in execution.get("checks", {}).items()
    }
    entry = {key: report.get(key) for key in ("updated_at", "fingerprint", "changed_paths", "tools_fingerprint")}
    entry.update(version=1, execution=execution)
    path = directory / "timing-history.jsonl"
    with path.open("a") as output:
        output.write(json.dumps(entry, ensure_ascii=False, separators=(",", ":")) + "\n")


def render(root, limit=8):
    directory = root / "target/verification"
    report = json.loads((directory / "report.json").read_text())
    policy = json.loads((root / "scripts/verification.json").read_text())
    lines = [
        "Verification timing diagnostics (not evidence of current passes)",
        "Contract inventory: " + json.dumps(inventory(policy), sort_keys=True),
    ]
    execution = report.get("execution", {})
    lines.append(
        f"Latest invocation: {execution.get('seconds', 'unknown')} s; state={execution.get('status', 'legacy')}"
    )
    # Legacy reports have only the scheduled order and last per-check durations.
    checks = execution.get("checks")
    if checks is None:
        lines.append("Legacy timings: last stored durations of scheduled checks; not a measured invocation total.")
        checks = {name: report["results"].get(name, {}) for name in execution.get("order", [])}
    for name, result in sorted(checks.items(), key=lambda pair: pair[1].get("seconds", 0), reverse=True):
        lines.append(f"  {name}: {result.get('seconds', 'unknown')} s ({result.get('status', 'unknown')})")
    if execution.get("resources_start"):
        lines.append("Resources at start: " + json.dumps(execution["resources_start"], sort_keys=True))
    lines.append("Reused checks this invocation: " + ", ".join(execution.get("reused", [])))
    contracts_path = directory / "product-contracts.json"
    if contracts_path.is_file():
        contracts = json.loads(contracts_path.read_text())
        lines.append(
            f"Last contract run: reused={contracts.get('reused', 0)}; "
            + json.dumps(contracts.get("execution", {}), sort_keys=True)
        )
        # These logs may belong to different runs. Never sum their time into the latest invocation.
        seconds = []
        for item in contracts.get("negative_controls", []):
            if item.get("reused"):
                continue
            path = directory / "product-contracts" / (item["id"] + "-negative.log")
            if path.is_file():
                seconds += [
                    float(value) for value in re.findall(r"Build complete! \(([0-9.]+)(?:s|秒)\)", path.read_text())
                ]
        if seconds:
            lines.append(
                f"Swift build log samples (last non-reused controls, overlapping work): n={len(seconds)}, "
                f"mean={sum(seconds) / len(seconds):.3f} s, max={max(seconds):.3f} s"
            )
    path = directory / "timing-history.jsonl"
    if path.is_file():
        lines.append("Recent invocations (different inputs/load/cache are not like-for-like benchmarks):")
        for row in path.read_text().splitlines()[-limit:]:
            try:
                item = json.loads(row)
                run = item["execution"]
                lines.append(
                    f"  {item['updated_at']}: {run.get('seconds')} s; {run.get('status')}; "
                    f"ran={len(run.get('checks', {}))}, reused={len(run.get('reused', []))}; "
                    f"source={str(item.get('fingerprint'))[:12]}"
                )
            except (ValueError, KeyError, TypeError):
                lines.append("  incomplete history row (interrupted write)")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--history", type=int, default=8)
    args = parser.parse_args()
    try:
        print(render(ROOT, max(1, args.history)))
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.exit(1, f"Timing diagnostics unavailable: {error}\n")


if __name__ == "__main__":
    main()
