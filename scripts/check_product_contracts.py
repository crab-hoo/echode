#!/usr/bin/env python3
"""Run required behavioral regressions; zero tests or skips are failures."""

from contextlib import contextmanager
import errno
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent


def run_process(args, *, cwd, capture_output, text, timeout, env=None):
    # Keep descendants in the outer verifier's isolated group, just like the
    # sanitizer driver. Its watchdog must be able to stop the entire check.
    supervised = os.environ.get("ECHODE_VERIFY_GROUP") == "1" and os.getsid(0) == os.getpgrp()
    process = subprocess.Popen(args, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               text=text, env=env, start_new_session=not supervised)
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        if supervised:
            print("product contract timed out; stopping the verification group", file=sys.stderr, flush=True)
            os.killpg(os.getpgrp(), signal.SIGKILL)
            raise
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.communicate()
        raise
    return subprocess.CompletedProcess(args, process.returncode, stdout, stderr)


def command(contract):
    name = contract["test"]
    if contract["runner"] == "rust":
        return ["cargo", "test", "--locked", "--offline", "-p", contract["package"],
                *contract["target"], name, "--", "--exact", "--nocapture", "--color", "never"]
    if contract["runner"] == "swift":
        return ["swift", "test", "--package-path", contract["package_path"],
                "--scratch-path", "target/echode-app-tests", "-Xswiftc", "-warnings-as-errors",
                "--filter", contract["suite"] + "/" + name]
    raise ValueError("unknown contract runner: " + contract["runner"])


def passed(contract, returncode, output):
    if returncode != 0:
        return False
    name = re.escape(contract["test"])
    if contract["runner"] == "rust":
        return bool(re.search(r"^test " + name + r" \.\.\. ok$", output, re.MULTILINE)
                    and re.search(r"test result: ok\. 1 passed; 0 failed; 0 ignored;", output))
    # Swift Testing is separate from the empty XCTest summary SwiftPM also emits.
    return bool(re.search(r"Test " + name + r"\(\) passed", output)
                and re.search(r"Test run with 1 test .*passed", output)
                and "known issue" not in output.lower())


def rejected_behavior(contract, returncode, output):
    """Compilation/tool failures do not prove the assertions catch a defect."""
    if returncode == 0:
        return False
    name = re.escape(contract["test"])
    if contract["runner"] == "rust":
        return bool(re.search(r"^test " + name + r" \.\.\. FAILED$", output, re.MULTILINE)
                    and "0 passed; 1 failed; 0 ignored;" in output)
    return bool(re.search(r"Test " + name + r"\(\) failed", output)
                and re.search(r"Test run with 1 test .*failed", output)
                and "Expectation failed" in output)


def snapshot(root, destination):
    # Copy the current source, including non-ignored untracked files. Do not
    # mutate the checkout or git index; all counterexamples run in this copy.
    paths = subprocess.check_output(["git", "ls-files", "-z", "--cached", "--others",
                                     "--exclude-standard"], cwd=root).split(b"\0")
    for raw in paths:
        if not raw:
            continue
        path = Path(os.fsdecode(raw))
        source, target = root / path, destination / path
        if not source.exists() and not source.is_symlink():
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        if source.is_symlink():
            target.symlink_to(os.readlink(source))
        else:
            shutil.copy2(source, target)
    # Swift's product wrapper links the already-built Rust ABI. The header is
    # generated/ignored, so neither it nor build products came from ls-files.
    (destination / "target").mkdir(exist_ok=True)
    (destination / "target/debug").symlink_to(root / "target/debug", target_is_directory=True)
    header = Path("swift/EchodeBridge/CEchodeFfi/include/echode.h")
    (destination / header).parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(root / header, destination / header)


def mutation_path(checkout, filename):
    path = (checkout / filename).resolve()
    if not path.is_relative_to(checkout.resolve()) or not path.is_file():
        raise ValueError("negative control must stay inside its source copy")
    return path


@contextmanager
def source_copy(root):
    directory = Path(tempfile.mkdtemp(prefix=".product-contracts-", dir=root / "target"))
    try:
        yield directory
    finally:
        # Finder can recreate .DS_Store between traversal and rmdir. Retry only
        # that directory-content race; permission errors and persistent failures
        # must still fail verification. This path belongs to this invocation.
        for attempt in range(3):
            try:
                shutil.rmtree(directory)
                break
            except OSError as error:
                if error.errno != errno.ENOTEMPTY or attempt == 2:
                    raise


def negative_controls(root, contracts):
    evidence = root / "target/verification/product-contracts"
    cargo_target = root / "target/product-contract-mutations/rust"
    (cargo_target / "debug").mkdir(parents=True, exist_ok=True)
    shutil.copy2(root / "target/debug/libechode_test_plugin.dylib",
                 cargo_target / "debug/libechode_test_plugin.dylib")
    shutil.copytree(root / "target/debug/plugins", cargo_target / "debug/plugins", dirs_exist_ok=True)
    results = []
    with source_copy(root) as checkout:
        snapshot(root, checkout)
        for contract in contracts:
            fault = contract["negative_control"]
            path = mutation_path(checkout, fault["file"])
            original = path.read_text()
            if original.count(fault["old"]) != 1:
                raise ValueError("negative control no longer matches: " + contract["id"])
            path.write_text(original.replace(fault["old"], fault["new"], 1))
            try:
                args = command(contract)
                env = dict(os.environ, CARGO_TARGET_DIR=str(cargo_target))
                completed = run_process(args, cwd=checkout, capture_output=True, text=True,
                                        timeout=contract["timeout"], env=env)
                output = completed.stdout + completed.stderr
                caught = rejected_behavior(contract, completed.returncode, output)
            except (OSError, subprocess.TimeoutExpired) as error:
                output, caught = str(error), False
            finally:
                path.write_text(original)
            log = evidence / (contract["id"] + "-negative.log")
            log.write_text(output)
            results.append({"id": contract["id"], "status": "passed" if caught else "failed",
                            "log": str(log), "purpose": "The test must reject the reintroduced defect"})
            if not caught:
                print(f"{contract['id']} negative control: FAILED", flush=True)
                print(output[-5000:], file=sys.stderr)
    return results


def run(root, contracts, invoke=run_process):
    if not contracts or len({c["id"] for c in contracts}) != len(contracts):
        raise ValueError("required product contracts must be nonempty and unique")
    directory = root / "target/verification/product-contracts"
    directory.mkdir(parents=True, exist_ok=True)
    report = directory.parent / "product-contracts.json"
    report.write_text(json.dumps({"status": "running", "tests": [], "negative_controls": []}) + "\n")
    results = []
    for contract in contracts:
        name = contract["id"]
        if not re.fullmatch(r"[a-z][a-z0-9-]*", name):
            raise ValueError("invalid contract id")
        log = directory / (name + ".log")
        args = command(contract)
        try:
            completed = invoke(args, cwd=root, capture_output=True, text=True,
                               timeout=contract["timeout"])
            output = completed.stdout + completed.stderr
            ok = passed(contract, completed.returncode, output)
            code = completed.returncode
        except (OSError, subprocess.TimeoutExpired) as error:
            output, ok, code = str(error), False, None
        log.write_text(output)
        results.append({"id": name, "status": "passed" if ok else "failed", "exit_code": code,
                        "test": contract["test"], "command": args, "log": str(log),
                        "purpose": contract["purpose"]})
        if not ok:
            print(f"{name}: FAILED (missing, skipped, or failed required test)", flush=True)
            print(output[-5000:], file=sys.stderr)
    controls = negative_controls(root, contracts) if all(r["status"] == "passed" for r in results) else []
    ok = all(r["status"] == "passed" for r in results + controls)
    if ok:
        print(f"product contracts: {len(results)} passed, {len(controls)} negative controls rejected defects", flush=True)
    report.write_text(json.dumps({"status": "passed" if ok else "failed",
                                  "tests": results, "negative_controls": controls}, indent=2) + "\n")
    return 0 if ok else 1


def main():
    try:
        policy = json.loads((ROOT / "scripts/verification.json").read_text())
        return run(ROOT, policy["product_contracts"])
    except (OSError, ValueError, KeyError) as error:
        report = ROOT / "target/verification/product-contracts.json"
        report.parent.mkdir(parents=True, exist_ok=True)
        report.write_text(json.dumps({"status": "failed", "error": str(error)}) + "\n")
        print("product contract verification failed: " + str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
