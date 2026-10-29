#!/usr/bin/env python3
"""Run required behavioral regressions; zero tests or skips are failures."""

from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack, contextmanager
import errno
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import time

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
HEADER = Path("swift/EchodeBridge/CEchodeFfi/include/echode.h")
SWIFT_SCRATCH = "target/echode-app-tests"
MUTATIONS = Path("target/product-contract-mutations")
# Swift counterexamples build this many copies at once, then run their tests one
# at a time with no build running. Each copy keeps its own source and scratch.
SWIFT_BUILD_WORKERS = 3


def load_verify():
    spec = importlib.util.spec_from_file_location("echode_verify", Path(__file__).resolve().parent / "verify.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


verify = load_verify()


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


def swift_package(contract):
    return ["--package-path", contract["package_path"], "--scratch-path", SWIFT_SCRATCH,
            "-Xswiftc", "-warnings-as-errors"]


def command(contract, skip_build=False):
    name = contract["test"]
    if contract["runner"] == "rust":
        return ["cargo", "test", "--locked", "--offline", "-p", contract["package"],
                *contract["target"], name, "--", "--exact", "--nocapture", "--color", "never"]
    if contract["runner"] == "swift":
        return ["swift", "test", *(["--skip-build"] if skip_build else []), *swift_package(contract),
                "--filter", contract["suite"] + "/" + name]
    raise ValueError("unknown contract runner: " + contract["runner"])


def swift_build(contract):
    # Same package, scratch path and flags as `swift test`; tests then run with --skip-build.
    selection = ["--product", contract["build_product"]] if contract.get("build_product") else ["--build-tests"]
    return ["swift", "build", *selection, *swift_package(contract)]


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


def negative_control_mutations(contract):
    """A contract may carry one mutation or several independent counterexamples."""
    return contract.get("negative_controls") or [contract["negative_control"]]


def contract_inputs(root, policy, contract):
    """Patterns and package folders one contract can depend on; a subset of the check's inputs."""
    groups = policy["product_contract_inputs"]
    patterns = policy["global_inputs"] + policy["evidence_inputs"]
    packages = []
    for group in ("common", contract["runner"]):
        patterns += groups[group]["inputs"]
        packages += groups[group]["packages"]
    if contract["runner"] == "rust":
        packages.append(contract["package"])
    folders = verify.package_directories(root, sorted(set(packages)))
    for fault in negative_control_mutations(contract):
        path = fault["file"]
        if not (verify.matches([path], patterns) or any(path.startswith(folder + "/") for folder in folders)):
            raise ValueError("negative control is outside its contract's inputs: " + contract["id"])
    return patterns, folders


def contract_fingerprints(root, policy, contracts, tools):
    """Per-contract evidence identities: an unrelated edit does not rerun a contract or its counterexample."""
    files = verify.source_files(root)
    check = next(item for item in policy["checks"] if item["id"] == "check-product-contracts")
    makefile = root / "Makefile"
    # The check's entry and recipe, but not the other contracts listed in the policy.
    recipe = verify.makefile_identity(verify.makefile_rules(makefile.read_text() if makefile.is_file() else ""),
                                      check.get("make_target", check["id"]))
    # The wrapper's scheduling/overall budget is not an input to individual test processes.
    shared = [recipe, policy["product_contract_inputs"], policy["global_inputs"], policy["evidence_inputs"]]
    stamps = {}
    for contract in contracts:
        patterns, folders = contract_inputs(root, policy, contract)
        configuration = [contract, shared, tools]
        stamps[contract["id"]] = verify.scoped_digest(root, files, configuration, patterns, folders)
    return stamps


def reusable_results(report, fingerprints):
    """Previous passes whose test and counterexample both ran on the same contract inputs."""
    try:
        tests = {item["id"]: item for item in report.get("tests", [])}
        controls = {item["id"]: item for item in report.get("negative_controls", [])}
    except (AttributeError, KeyError, TypeError):
        return {}
    reusable = {}
    for name, stamp in fingerprints.items():
        test, control = tests.get(name, {}), controls.get(name, {})
        if (stamp and test.get("status") == "passed" and control.get("status") == "passed"
                and test.get("fingerprint") == stamp and control.get("fingerprint") == stamp):
            reusable[name] = (dict(test, reused=True), dict(control, reused=True))
    return reusable


def same_file(source, target):
    try:
        return (not target.is_symlink() and target.is_file() and source.stat().st_mode == target.stat().st_mode
                and source.stat().st_size == target.stat().st_size and source.read_bytes() == target.read_bytes())
    except OSError:
        return False


def remove_path(path):
    if path.is_symlink() or path.is_file():
        path.unlink()
    elif path.is_dir():
        # Finder can recreate .DS_Store between traversal and rmdir. Retry only
        # that directory-content race; other failures must still stop the check.
        for attempt in range(3):
            try:
                shutil.rmtree(path)
                break
            except OSError as error:
                if error.errno != errno.ENOTEMPTY or attempt == 2:
                    raise


def sync_checkout(root, checkout):
    """Make a persistent copy match the current source without touching unchanged files.

    Unchanged files keep their old times, so Cargo and SwiftPM reuse earlier builds.
    Every changed or restored file gets a new time, so a build from an earlier
    counterexample is never mistaken for one from the current source.
    """
    manifest = checkout.with_name(checkout.name + ".json")
    try:
        previous = set(json.loads(manifest.read_text())["paths"]) if checkout.is_dir() else set()
    except (OSError, ValueError, KeyError, TypeError):
        # Unknown contents could hold stale sources that SwiftPM would compile.
        previous = None
    if previous is None:
        for child in checkout.iterdir():
            if child.name != "target":
                remove_path(child)
        previous = set()
    checkout.mkdir(parents=True, exist_ok=True)
    current = [relative for relative in verify.source_files(root)
               if (root / relative).exists() or (root / relative).is_symlink()]
    current.append(HEADER.as_posix())
    # Record the union first: an interrupted sync can still delete what it created.
    manifest.write_text(json.dumps({"paths": sorted(previous | set(current))}) + "\n")
    for relative in current:
        source, target = root / relative, checkout / relative
        if source.is_symlink():
            if target.is_symlink() and os.readlink(target) == os.readlink(source):
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            remove_path(target)
            target.symlink_to(os.readlink(source))
        elif not same_file(source, target):
            target.parent.mkdir(parents=True, exist_ok=True)
            if target.is_symlink() or target.is_dir():
                remove_path(target)
            # Not copy2: an older time from the source must not look like an earlier build input.
            shutil.copyfile(source, target)
            shutil.copymode(source, target)
    for relative in sorted(previous - set(current), reverse=True):
        target = checkout / relative
        remove_path(target)
        for parent in target.parents:
            if parent == checkout:
                break
            try:
                parent.rmdir()
            except OSError:
                break
    # Swift's product wrapper links the already-built Rust ABI from the real tree.
    debug = checkout / "target/debug"
    if not (debug.is_symlink() and Path(os.readlink(debug)) == root / "target/debug"):
        debug.parent.mkdir(parents=True, exist_ok=True)
        remove_path(debug)
        debug.symlink_to(root / "target/debug", target_is_directory=True)
    manifest.write_text(json.dumps({"paths": sorted(current)}) + "\n")


def mutation_path(checkout, filename):
    path = (checkout / filename).resolve()
    if not path.is_relative_to(checkout.resolve()) or not path.is_file():
        raise ValueError("negative control must stay inside its source copy")
    return path


@contextmanager
def mutated(checkout, contract, fault):
    path = mutation_path(checkout, fault["file"])
    original = path.read_text()
    if original.count(fault["old"]) != 1:
        raise ValueError("negative control no longer matches: " + contract["id"])
    path.write_text(original.replace(fault["old"], fault["new"], 1))
    try:
        yield
    finally:
        # A new write time makes the next build recompile the restored file.
        path.write_text(original)


@contextmanager
def mutation_lock(root):
    directory = root / MUTATIONS
    directory.mkdir(parents=True, exist_ok=True)
    with (directory / ".lock").open("a") as lock:
        deadline = time.monotonic() + 30
        while True:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise ValueError("another product contract run holds " + str(MUTATIONS / ".lock"))
                time.sleep(0.1)
        try:
            yield directory
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def mutation_order(root, contracts):
    """Rust counterexamples grouped by the crate they change, dependencies first.

    Each restored crate is then rebuilt once per group instead of after every
    counterexample, and the last group leaves only a downstream crate stale.
    """
    members = {name: folder.resolve().relative_to(root.resolve()).as_posix()
               for name, folder in verify.workspace_members(root).items()}
    depth = {name: len(verify.package_directories(root, [name], ("dependencies", "build-dependencies")))
             for name in members}

    def owner(contract):
        path = negative_control_mutations(contract)[0]["file"]
        names = [name for name, folder in members.items() if path.startswith(folder + "/")]
        return max(names, key=lambda name: len(members[name])) if names else ""

    indexed = list(enumerate(contracts))
    return [contract for _, contract in sorted(indexed, key=lambda item: (
        depth.get(owner(item[1]), 0), owner(item[1]), item[1]["package"], item[1]["target"], item[0]))]


def control_result(contract, stamp, caught, log, seconds):
    return {"id": contract["id"], "status": "passed" if caught else "failed", "log": str(log),
            "purpose": "The test must reject the reintroduced defect", "fingerprint": stamp,
            "seconds": round(seconds, 3)}


def write_control(evidence, contract, stamp, output, caught, started, record):
    log = evidence / (contract["id"] + "-negative.log")
    log.write_text(output)
    record(control_result(contract, stamp, caught, log, time.monotonic() - started))
    if not caught:
        print(f"{contract['id']} negative control: FAILED", flush=True)
        print(output[-5000:], file=sys.stderr)


def rust_controls(checkout, contracts, fingerprints, cargo_target, evidence, record):
    env = dict(os.environ, CARGO_TARGET_DIR=str(cargo_target))
    for contract in contracts:
        started = time.monotonic()
        output = ""
        caught = True
        for index, fault in enumerate(negative_control_mutations(contract), 1):
            try:
                with mutated(checkout, contract, fault):
                    completed = run_process(command(contract), cwd=checkout, capture_output=True, text=True,
                                            timeout=contract["timeout"], env=env)
                run_output = completed.stdout + completed.stderr
                rejected = rejected_behavior(contract, completed.returncode, run_output)
            except (OSError, subprocess.TimeoutExpired) as error:
                run_output, rejected = str(error), False
            output += f"mutation {index}/{len(negative_control_mutations(contract))}: " \
                      f"{'rejected' if rejected else 'NOT rejected'}\n" + run_output
            caught = caught and rejected
        write_control(evidence, contract, fingerprints.get(contract["id"]), output, caught, started, record)


def swift_mutation_jobs(contracts):
    """Identical source + package + build selector can share a build, never a test process."""
    jobs = {}
    for contract in contracts:
        for index, fault in enumerate(negative_control_mutations(contract)):
            key = (contract["package_path"], contract.get("build_product"), fault["file"], fault["old"], fault["new"])
            jobs.setdefault(key, []).append((contract, index, fault))
    # Keep changes to the same file together to avoid rebuilding restored, unrelated files.
    return sorted(jobs.values(), key=lambda job: (job[0][0]["package_path"], job[0][2]["file"]))


def swift_controls(checkouts, contracts, fingerprints, evidence, record):
    def build(checkout, job):
        contract = min((item[0] for item in job), key=lambda item: item["timeout"])
        started = time.monotonic()
        try:
            completed = run_process(swift_build(contract), cwd=checkout, capture_output=True, text=True,
                                    timeout=contract["timeout"])
            return completed.returncode, completed.stdout + completed.stderr, time.monotonic() - started
        except (OSError, subprocess.TimeoutExpired) as error:
            return None, str(error), time.monotonic() - started

    state = {c["id"]: {"output": "", "caught": True, "done": 0, "seconds": 0.0} for c in contracts}
    jobs = swift_mutation_jobs(contracts)
    timings = {"builds": len(jobs), "mutations": sum(len(job) for job in jobs),
               "build_work_seconds": 0.0, "test_seconds": 0.0}
    started = time.monotonic()
    for start in range(0, len(jobs), len(checkouts)):
        group = list(zip(checkouts, jobs[start:start + len(checkouts)]))
        with ExitStack() as restore:
            for checkout, job in group:
                contract, _, fault = job[0]
                restore.enter_context(mutated(checkout, contract, fault))
            with ThreadPoolExecutor(len(group)) as pool:
                builds = list(pool.map(lambda pair: build(*pair), group))
            # All builds finish before any test starts. Each test has its own process and deadline.
            for (checkout, job), (code, build_output, seconds) in zip(group, builds):
                timings["build_work_seconds"] += seconds
                for contract, index, _ in job:
                    output, rejected = build_output, False
                    remaining = contract["timeout"] - seconds
                    test_started = time.monotonic()
                    if code == 0 and remaining > 0:
                        try:
                            completed = run_process(command(contract, skip_build=True), cwd=checkout,
                                                    capture_output=True, text=True, timeout=remaining)
                            output += completed.stdout + completed.stderr
                            rejected = rejected_behavior(contract, completed.returncode, output)
                        except (OSError, subprocess.TimeoutExpired) as error:
                            output += str(error)
                    elif code == 0:
                        output += "\nthe counterexample build used the whole contract timeout\n"
                    test_seconds = time.monotonic() - test_started
                    timings["test_seconds"] += test_seconds
                    entry = state[contract["id"]]
                    entry["seconds"] += seconds + test_seconds
                    entry["output"] += f"mutation {index + 1}/{len(negative_control_mutations(contract))}: " \
                                       f"{'rejected' if rejected else 'NOT rejected'}\n" + output
                    entry["caught"] = entry["caught"] and rejected
                    entry["done"] += 1
                    if entry["done"] == len(negative_control_mutations(contract)):
                        # Duration is this contract's work, not time spent queued behind earlier contracts.
                        log = evidence / (contract["id"] + "-negative.log")
                        log.write_text(entry["output"])
                        record(control_result(contract, fingerprints.get(contract["id"]), entry["caught"],
                                              log, entry["seconds"]))
                        if not entry["caught"]:
                            print(f"{contract['id']} negative control: FAILED", flush=True)
                            print(entry["output"][-5000:], file=sys.stderr)
    return {**timings, "seconds": round(time.monotonic() - started, 3)}


def negative_controls(root, contracts, fingerprints, evidence, record):
    started = time.monotonic()
    with mutation_lock(root) as directory:
        # Only this runner builds here; another writer's times could make a
        # counterexample build look current (the pre-ADR-0148 copies used "rust").
        cargo_target = directory / "cargo"
        (cargo_target / "debug").mkdir(parents=True, exist_ok=True)
        shutil.copy2(root / "target/debug/libechode_test_plugin.dylib",
                     cargo_target / "debug/libechode_test_plugin.dylib")
        shutil.copytree(root / "target/debug/plugins", cargo_target / "debug/plugins", dirs_exist_ok=True)
        swift = [contract for contract in contracts if contract["runner"] == "swift"]
        rust = mutation_order(root, [contract for contract in contracts if contract["runner"] == "rust"])
        count = max(1, min(SWIFT_BUILD_WORKERS, len(swift))) if swift else 1
        checkouts = [directory / "checkouts" / str(index) for index in range(count)]
        for checkout in checkouts:
            sync_checkout(root, checkout)
        sync_seconds = time.monotonic() - started
        rust_started = time.monotonic()
        rust_controls(checkouts[0], rust, fingerprints, cargo_target, evidence, record)
        rust_seconds = time.monotonic() - rust_started
        swift = swift_controls(checkouts, swift, fingerprints, evidence, record)
        return {"sync_seconds": round(sync_seconds, 3), "rust_seconds": round(rust_seconds, 3), "swift": swift}


def build_swift_package(root, contracts, contract, invoke):
    # One build serves every test of the package; each test still runs in its own process.
    timeout = max(item["timeout"] for item in contracts
                  if (item.get("package_path"), item.get("build_product"))
                  == (contract["package_path"], contract.get("build_product")))
    try:
        built = invoke(swift_build(contract), cwd=root, capture_output=True, text=True, timeout=timeout)
        return built.returncode == 0, built.stdout + built.stderr
    except (OSError, subprocess.TimeoutExpired) as error:
        return False, str(error)


def run_tests(root, contracts, invoke, fingerprints, evidence, record):
    builds = {}
    for contract in contracts:
        name = contract["id"]
        if not re.fullmatch(r"[a-z][a-z0-9-]*", name):
            raise ValueError("invalid contract id")
        log = evidence / (name + ".log")
        started = time.monotonic()
        args = command(contract, skip_build=contract["runner"] == "swift")
        built = True
        if contract["runner"] == "swift":
            package = contract["package_path"]
            key = (package, contract.get("build_product"))
            first = key not in builds
            if first:
                builds[key] = build_swift_package(root, contracts, contract, invoke)
            built, output = builds[key]
            ok, code = False, None
            if not built and not first:
                output = f"Swift test-product build failed for {package}; see the first Swift contract log\n"
        if built:
            try:
                completed = invoke(args, cwd=root, capture_output=True, text=True, timeout=contract["timeout"])
                output = completed.stdout + completed.stderr
                ok = passed(contract, completed.returncode, output)
                code = completed.returncode
            except (OSError, subprocess.TimeoutExpired) as error:
                output, ok, code = str(error), False, None
        log.write_text(output)
        record({"id": name, "status": "passed" if ok else "failed", "exit_code": code,
                "test": contract["test"], "command": args, "log": str(log), "purpose": contract["purpose"],
                "fingerprint": fingerprints.get(name), "seconds": round(time.monotonic() - started, 3)})
        if not ok:
            print(f"{name}: FAILED (missing, skipped, or failed required test)", flush=True)
            print(output[-5000:], file=sys.stderr)


def run(root, contracts, invoke=run_process, policy=None, fresh=False, evidence=None):
    started = time.monotonic()
    execution = {}
    if not contracts or len({c["id"] for c in contracts}) != len(contracts):
        raise ValueError("required product contracts must be nonempty and unique")
    directory = (evidence or root / "target/verification") / "product-contracts"
    directory.mkdir(parents=True, exist_ok=True)
    report = directory.parent / "product-contracts.json"
    try:
        previous = json.loads(report.read_text())
    except (OSError, ValueError):
        previous = {}
    fingerprints = (contract_fingerprints(root, policy, contracts, verify.tool_identity(root))
                    if policy is not None else {})
    reused = {} if fresh else reusable_results(previous, fingerprints)
    pending = [contract for contract in contracts if contract["id"] not in reused]
    tests = {name: pair[0] for name, pair in reused.items()}
    controls = {name: pair[1] for name, pair in reused.items()}

    def save(status):
        # Written after every result, so an interrupted run keeps the evidence it finished.
        order = [contract["id"] for contract in contracts]
        report.write_text(json.dumps({"status": status, "reused": len(reused),
                                      "execution": {**execution, "seconds": round(time.monotonic() - started, 3)},
                                      "tests": [tests[name] for name in order if name in tests],
                                      "negative_controls": [controls[name] for name in order if name in controls]},
                                     indent=2) + "\n")

    def record_test(result):
        tests[result["id"]] = result
        save("running")

    def record_control(result):
        controls[result["id"]] = result
        execution["negative_progress"] = {
            "seconds": round(time.monotonic() - control_started, 3),
            "completed": {runner: sum(c["id"] in controls and c["id"] not in reused
                                      for c in contracts if c["runner"] == runner)
                          for runner in ("rust", "swift")}}
        save("running")

    execution["identity_seconds"] = round(time.monotonic() - started, 3)
    execution["pending"] = len(pending)
    save("running")
    normal_started = time.monotonic()
    run_tests(root, pending, invoke, fingerprints, directory, record_test)
    execution["normal_seconds"] = round(time.monotonic() - normal_started, 3)
    if pending and all(tests[contract["id"]]["status"] == "passed" for contract in pending):
        control_started = time.monotonic()
        execution["negative"] = negative_controls(root, pending, fingerprints, directory, record_control)
        execution["negative_seconds"] = round(time.monotonic() - control_started, 3)
    if policy is not None and pending:
        # A result is tied to the inputs it ran on: an edit during the run makes it stale, never reusable.
        after = contract_fingerprints(root, policy, contracts, verify.tool_identity(root))
        for name in [contract["id"] for contract in pending if after[contract["id"]] != fingerprints[contract["id"]]]:
            for entries in (tests, controls):
                if name in entries:
                    entries[name] = dict(entries[name], status="stale", fingerprint=None)
            print(f"{name}: inputs changed during the run; run it again", file=sys.stderr, flush=True)
    ok = all(tests.get(c["id"], {}).get("status") == "passed" and controls.get(c["id"], {}).get("status") == "passed"
             for c in contracts)
    if ok:
        print(f"product contracts: {len(tests)} passed, {len(controls)} negative controls rejected defects"
              f" ({len(reused)} reused for unchanged inputs)", flush=True)
    save("passed" if ok else "failed")
    return 0 if ok else 1


def main():
    try:
        policy = verify.load_policy(ROOT)
        return run(ROOT, policy["product_contracts"], policy=policy,
                   fresh=os.environ.get("ECHODE_VERIFY_FORCE") == "1")
    except (OSError, ValueError, KeyError) as error:
        report = ROOT / "target/verification/product-contracts.json"
        report.parent.mkdir(parents=True, exist_ok=True)
        report.write_text(json.dumps({"status": "failed", "error": str(error)}) + "\n")
        print("product contract verification failed: " + str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
