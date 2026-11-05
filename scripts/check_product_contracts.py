#!/usr/bin/env python3
"""Run required behavioral regressions; zero tests or skips are failures."""

from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack, contextmanager
import errno
import fcntl
import hashlib
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
# A copy can be many edits behind the source. Bringing it up to date is not part of any counterexample.
SWIFT_COPY_BUILD_TIMEOUT = 1200
# Lines around a mutated text that belong to its site, on each side.
SITE_CONTEXT_LINES = 20
# `I/N` runs every N-th counterexample from I, so CI jobs split the full tier (ADR 0269).
SHARD_VARIABLE = "ECHODE_CONTRACT_SHARD"


def load_verify():
    spec = importlib.util.spec_from_file_location("echode_verify", Path(__file__).resolve().parent / "verify.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


verify = load_verify()
from swift_contract_runner import batch_invocation as swift_batch_invocation
from swift_contract_runner import invocation as swift_invocation


def test_invocation(root, contract):
    if contract["runner"] == "swift" and contract.get("build_product") == "EchodeAppTests":
        return swift_invocation(root, contract, SWIFT_SCRATCH)
    return command(contract, skip_build=contract["runner"] == "swift"), None


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


# Argument-bearing functions prove coverage only after at least one case ran.
SWIFT_SIGNATURE = r"(?:\(\)|\([^)\r\n]+\) with [1-9][0-9]* test cases?)"


def swift_test_result(test, outcome, output):
    return bool(re.search(r"Test " + re.escape(test) + SWIFT_SIGNATURE + " " + outcome, output))


def batch_key(contract):
    """The test process that can prove this contract together with others, or None for its own process."""
    if contract["runner"] == "rust":
        return ("rust", contract["package"], *contract["target"])
    if contract["runner"] == "swift" and contract.get("build_product") == "EchodeAppTests":
        return ("swift", contract["package_path"], contract["build_product"])
    return None


def batch_command(contracts):
    """One libtest run of every named test in one binary. Without --nocapture each result line stays whole."""
    first = contracts[0]
    names = list(dict.fromkeys(contract["test"] for contract in contracts))
    return ["cargo", "test", "--locked", "--offline", "-p", first["package"], *first["target"], "--",
            *names, "--exact", "--color", "never"]


def proven_in_batch(contract, output):
    """This test's own single pass line, from a batch that ran to its summary.

    An ignored, skipped, failed, missing or doubly matched test is not proven; it then runs alone.
    """
    if contract["runner"] == "rust":
        results = re.findall(r"^test " + re.escape(contract["test"]) + r" \.\.\. (\S+)$", output, re.MULTILINE)
        return results == ["ok"] and re.search(r"^test result: ", output, re.MULTILINE) is not None
    results = [match.group(0) for match in re.finditer(
        r"Test " + re.escape(contract["test"]) + SWIFT_SIGNATURE + r" (?:passed|failed|skipped)[^\r\n]*", output)]
    return (len(results) == 1 and " passed" in results[0] and "known issue" not in results[0]
            and re.search(r"Test run with [1-9][0-9]* tests?", output) is not None)


def passed(contract, returncode, output):
    if returncode != 0:
        return False
    name = re.escape(contract["test"])
    if contract["runner"] == "rust":
        return bool(re.search(r"^test " + name + r" \.\.\. ok$", output, re.MULTILINE)
                    and re.search(r"test result: ok\. 1 passed; 0 failed; 0 ignored;", output))
    # Swift Testing is separate from the empty XCTest summary SwiftPM also emits.
    return bool(swift_test_result(contract["test"], "passed", output)
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
    return bool(swift_test_result(contract["test"], "failed", output)
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
    keys = ["common", contract["runner"]]
    if contract["runner"] == "rust" and "rust:" + contract["package"] in groups:
        keys.append("rust:" + contract["package"])
    for key in keys:
        patterns += groups[key]["inputs"]
        packages += groups[key]["packages"]
    if contract["runner"] == "rust":
        packages.append(contract["package"])
    folders = verify.package_directories(root, sorted(set(packages)))
    for fault in negative_control_mutations(contract):
        path = fault["file"]
        if not (verify.matches([path], patterns) or any(path.startswith(folder + "/") for folder in folders)):
            raise ValueError("negative control is outside its contract's inputs: " + contract["id"])
    return patterns, folders


def contract_fingerprints(root, policy, contracts, tools, phase="complete"):
    """Per-contract evidence identities: an unrelated edit does not rerun a contract or its counterexample."""
    files = verify.source_files(root)
    check = next(item for item in policy["checks"] if item["id"] == "check-product-contracts")
    makefile = root / "Makefile"
    # The check's entry and recipe, but not the other contracts listed in the policy.
    recipe = verify.makefile_identity(verify.makefile_rules(makefile.read_text() if makefile.is_file() else ""),
                                      check.get("make_target", check["id"]))
    # The wrapper's scheduling/overall budget is not an input to individual test processes.
    shared = [recipe, policy["global_inputs"], policy["evidence_inputs"], phase]
    stamps = {}
    scopes = {}
    for contract in contracts:
        patterns, folders = contract_inputs(root, policy, contract)
        definition = {key: value for key, value in contract.items()
                      if phase != "tests" or key not in ("negative_control", "negative_controls", "purpose")}
        test_folders = ({verify.workspace_members(root)[contract["package"]].relative_to(root).as_posix()}
                        if contract["runner"] == "rust" else set())
        excludes = verify.package_test_excludes(folders, test_folders, root)
        configuration = [definition, shared, patterns, sorted(folders), excludes, tools]
        stamps[contract["id"]] = verify.scoped_digest(root, files, configuration, patterns, folders,
                                                     package_excludes=excludes, scopes=scopes)
    return stamps


def mutation_site(text, fault):
    """The mutated text and its neighbours; an edit elsewhere in the file leaves this unchanged."""
    if text.count(fault["old"]) != 1:
        return None
    before, after = text.split(fault["old"])
    return ["\n".join(before.splitlines()[-SITE_CONTEXT_LINES:]), fault["old"],
            "\n".join(after.splitlines()[:SITE_CONTEXT_LINES])]


def test_body(text, contract):
    """The contract's test function, by the indentation of its closing brace."""
    # Rust names a test by its module path.
    name = contract["test"].rsplit("::", 1)[-1]
    if name not in text:
        return None
    keyword = "fn" if contract["runner"] == "rust" else "func"
    start = re.search(r"^([ \t]*)[^\n]*\b" + keyword + r"[ \t]+" + re.escape(name) + r"\b", text, re.MULTILINE)
    if start is None:
        return None
    end = re.compile("^" + start[1] + r"\}", re.MULTILINE).search(text, start.end())
    return text[start.start():end.end() if end else len(text)]


def shard_contracts(contracts, shard):
    """The ids one CI job checks. Every contract is in exactly one of the N shards."""
    match = re.fullmatch(r"(0|[1-9][0-9]*)/([1-9][0-9]*)", shard)
    if not match or int(match[1]) >= int(match[2]):
        raise ValueError(f"{SHARD_VARIABLE} must be I/N with 0 <= I < N, got {shard!r}")
    index, count = int(match[1]), int(match[2])
    return {contract["id"] for position, contract in enumerate(contracts) if position % count == index}


def control_definition(contract):
    return {key: value for key, value in contract.items() if key != "purpose"}


def read_current(root, path):
    try:
        return (root / path).read_text(errors="replace")
    except OSError:
        return ""


def read_at_base(root, path, base):
    try:
        return verify.git(root, "show", f"{base or 'HEAD'}:{path}").decode(errors="replace")
    except subprocess.CalledProcessError:
        return ""


def reject_unmatched_controls(root, contracts):
    """Every counterexample must still apply. This needs no build, so it runs on every check."""
    texts = {}
    unmatched = sorted({contract["id"] for contract in contracts for fault in negative_control_mutations(contract)
                        if texts.setdefault(fault["file"], read_current(root, fault["file"])).count(fault["old"]) != 1})
    if unmatched:
        raise ValueError("negative control no longer matches: " + ", ".join(unmatched))


def control_sites(root, policy, contracts, base=None):
    """What each counterexample directly depends on, and the contracts where that differs from the diff base.

    A control still proves its test catches the defect while its definition, the
    mutated text and the test body are unchanged. A distant edit can in principle
    change that too; the full tier reruns every control to catch those (ADR 0212).
    """
    sources = {path: (read_at_base(root, path, base), read_current(root, path))
               for path in verify.changed_paths(root, base)}
    try:
        previous = json.loads(read_at_base(root, verify.POLICY.as_posix(), base))["product_contracts"]
    except (ValueError, KeyError, TypeError):
        previous = []
    earlier = {contract["id"]: control_definition(contract) for contract in previous}
    suffixes = {"rust": ".rs", "swift": ".swift"}
    texts = {}
    sites, targets = {}, set()
    for contract in contracts:
        mutations = negative_control_mutations(contract)
        bodies = {}
        for path, (old, new) in sources.items():
            if path.endswith(suffixes.get(contract["runner"], "\0")):
                pair = (test_body(old, contract), test_body(new, contract))
                if pair != (None, None):
                    bodies[path] = pair
        current = [mutation_site(texts.setdefault(fault["file"], read_current(root, fault["file"])), fault)
                   for fault in mutations]
        sites[contract["id"]] = hashlib.sha256(json.dumps(
            [control_definition(contract), current, sorted((path, pair[1]) for path, pair in bodies.items())],
            sort_keys=True).encode()).hexdigest()
        if (earlier.get(contract["id"]) != control_definition(contract)
                or any(old != new for old, new in bodies.values())
                or any(fault["file"] in sources and mutation_site(sources[fault["file"]][0], fault) != site
                       for fault, site in zip(mutations, current))):
            targets.add(contract["id"])
    return sites, targets


def passed_controls(report):
    """Every passing counterexample on record, whichever inputs it ran on."""
    try:
        return {item["id"]: item for item in report.get("negative_controls", []) if item.get("status") == "passed"}
    except (AttributeError, KeyError, TypeError):
        return {}


def reusable_phase(report, phase, fingerprints):
    """Reuse only a complete, passing phase tied to its own current inputs."""
    try:
        entries = {item["id"]: item for item in report.get(phase, [])}
        return {name: dict(entries[name], reused=True) for name, stamp in fingerprints.items()
                if stamp and entries.get(name, {}).get("status") == "passed"
                and entries[name].get("fingerprint") == stamp}
    except (AttributeError, KeyError, TypeError):
        return {}


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


def rust_controls(checkout, contracts, fingerprints, cargo_target, evidence, record):
    env = dict(os.environ, CARGO_TARGET_DIR=str(cargo_target))
    # A single source transition can serve different test targets. Cargo still
    # selects each package independently, retaining its original feature graph.
    jobs = {}
    for contract in contracts:
        for index, fault in enumerate(negative_control_mutations(contract)):
            key = (fault["file"], fault["old"], fault["new"])
            jobs.setdefault(key, []).append((contract, index, fault))
    state = {c["id"]: {"output": "", "caught": True, "done": 0, "seconds": 0.0} for c in contracts}
    for job in jobs.values():
        with mutated(checkout, job[0][0], job[0][2]):
            for contract, index, _ in job:
                started = time.monotonic()
                try:
                    completed = run_process(command(contract), cwd=checkout, capture_output=True, text=True,
                                            timeout=contract["timeout"], env=env)
                    output = completed.stdout + completed.stderr
                    rejected = rejected_behavior(contract, completed.returncode, output)
                except (OSError, subprocess.TimeoutExpired) as error:
                    output, rejected = str(error), False
                entry = state[contract["id"]]
                entry["seconds"] += time.monotonic() - started
                entry["output"] += f"mutation {index + 1}/{len(negative_control_mutations(contract))}: " \
                                   f"{'rejected' if rejected else 'NOT rejected'}\n" + output
                entry["caught"] = entry["caught"] and rejected
                entry["done"] += 1
                if entry["done"] == len(negative_control_mutations(contract)):
                    log = evidence / (contract["id"] + "-negative.log")
                    log.write_text(entry["output"])
                    record(control_result(contract, fingerprints.get(contract["id"]), entry["caught"], log, entry["seconds"]))
                    if not entry["caught"]:
                        print(f"{contract['id']} negative control: FAILED", flush=True)
                        print(entry["output"][-5000:], file=sys.stderr)
    return {"source_transitions": len(jobs), "test_processes": sum(len(job) for job in jobs.values())}


def swift_mutation_jobs(contracts):
    """Identical source + package + build selector can share a build, never a test process."""
    jobs = {}
    for contract in contracts:
        for index, fault in enumerate(negative_control_mutations(contract)):
            key = (contract["package_path"], contract.get("build_product"), fault["file"], fault["old"], fault["new"])
            jobs.setdefault(key, []).append((contract, index, fault))
    # Keep changes to the same file together to avoid rebuilding restored, unrelated files.
    return sorted(jobs.values(), key=lambda job: (job[0][0]["package_path"], job[0][2]["file"]))


def build_swift_copies(checkouts, contracts):
    """Build the unmutated source in each copy, so a contract's deadline covers only its own change."""
    products = {(contract["package_path"], contract.get("build_product")): contract for contract in contracts}

    def build(checkout):
        for contract in products.values():
            completed = run_process(swift_build(contract), cwd=checkout, capture_output=True, text=True,
                                    timeout=SWIFT_COPY_BUILD_TIMEOUT)
            if completed.returncode != 0:
                raise ValueError("the unmutated source copy does not build:\n"
                                 + (completed.stdout + completed.stderr)[-5000:])

    with ThreadPoolExecutor(len(checkouts)) as pool:
        list(pool.map(build, checkouts))


def swift_controls(checkouts, contracts, fingerprints, evidence, record):
    def build(checkout, job):
        contract = min((item[0] for item in job), key=lambda item: item["timeout"])
        started = time.monotonic()
        try:
            completed = run_process(swift_build(contract), cwd=checkout, capture_output=True, text=True,
                                    timeout=contract["timeout"])
            if completed.returncode == 0:
                test_invocation(checkout, contract)
            return completed.returncode, completed.stdout + completed.stderr, time.monotonic() - started
        except (OSError, ValueError, subprocess.SubprocessError) as error:
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
                            args, env = test_invocation(checkout, contract)
                            completed = run_process(args, cwd=checkout,
                                                    capture_output=True, text=True, timeout=remaining, env=env)
                            output += completed.stdout + completed.stderr
                            rejected = rejected_behavior(contract, completed.returncode, output)
                        except (OSError, ValueError, subprocess.SubprocessError) as error:
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
        rust_timings = rust_controls(checkouts[0], rust, fingerprints, cargo_target, evidence, record)
        rust_seconds = time.monotonic() - rust_started
        copy_started = time.monotonic()
        if swift:
            build_swift_copies(checkouts, swift)
        copy_seconds = time.monotonic() - copy_started
        swift = swift_controls(checkouts, swift, fingerprints, evidence, record)
        return {"sync_seconds": round(sync_seconds, 3), "rust_seconds": round(rust_seconds, 3), "rust": rust_timings,
                "swift_copy_seconds": round(copy_seconds, 3), "swift": swift}


def build_swift_package(root, contracts, contract, invoke):
    # One build serves every test of the package; each test still runs in its own process.
    timeout = max(item["timeout"] for item in contracts
                  if (item.get("package_path"), item.get("build_product"))
                  == (contract["package_path"], contract.get("build_product")))
    try:
        built = invoke(swift_build(contract), cwd=root, capture_output=True, text=True, timeout=timeout)
        if built.returncode == 0:
            test_invocation(root, contract)
        return built.returncode == 0, built.stdout + built.stderr
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        return False, str(error)


def run_batches(root, contracts, invoke, fingerprints, evidence, record):
    """Prove a task's required tests with one process per test binary (ADR 0270). Returns the rest.

    The same tests already ran in check and check-swift; this keeps the evidence of each one
    without one process per test. The full tier still runs each test alone.
    """
    groups, alone = {}, []
    for contract in contracts:
        key = batch_key(contract)
        if key is None:
            alone.append(contract)
        else:
            groups.setdefault(key, []).append(contract)
    for key, group in groups.items():
        if len(group) == 1:
            alone += group
            continue
        started = time.monotonic()
        log = evidence / ("batch-" + re.sub(r"[^A-Za-z0-9_.-]+", "-", "-".join(key).strip("-")) + ".log")
        args, env, code = None, None, None
        try:
            if key[0] == "swift":
                built, output = build_swift_package(root, contracts, group[0], invoke)
                if not built:
                    log.write_text(output)
                    alone += group
                    continue
                args, env = swift_batch_invocation(root, group, SWIFT_SCRATCH)
            else:
                args = batch_command(group)
            completed = invoke(args, cwd=root, capture_output=True, text=True,
                               timeout=sum(contract["timeout"] for contract in group),
                               **({"env": env} if env is not None else {}))
            output, code = completed.stdout + completed.stderr, completed.returncode
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            output = str(error)
        log.write_text(output)
        seconds = round(time.monotonic() - started, 3)
        for contract in group:
            if proven_in_batch(contract, output):
                record({"id": contract["id"], "status": "passed", "exit_code": code, "test": contract["test"],
                        "command": args, "log": str(log), "purpose": contract["purpose"],
                        "fingerprint": fingerprints.get(contract["id"]), "seconds": seconds, "isolation": "batch"})
            else:
                alone.append(contract)
    return alone


def run_tests(root, contracts, invoke, fingerprints, evidence, record, batched=False):
    if batched:
        contracts = run_batches(root, contracts, invoke, fingerprints, evidence, record)
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
                args, env = test_invocation(root, contract)
                completed = invoke(args, cwd=root, capture_output=True, text=True, timeout=contract["timeout"], **({"env": env} if env is not None else {}))
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


def run(root, contracts, invoke=run_process, policy=None, fresh=False, evidence=None, changed_only=False, base=None,
        shard=None):
    """Every required test, then every counterexample or, with `changed_only`, those whose site changed.

    With `shard`, only that share of the counterexamples runs; every required test still runs.
    """
    started = time.monotonic()
    execution = {}
    if not contracts or len({c["id"] for c in contracts}) != len(contracts):
        raise ValueError("required product contracts must be nonempty and unique")
    if shard is not None and changed_only:
        raise ValueError(f"{SHARD_VARIABLE} splits the full tier; --changed-controls already narrows the controls")
    in_shard = shard_contracts(contracts, shard) if shard is not None else None
    directory = (evidence or root / "target/verification") / "product-contracts"
    directory.mkdir(parents=True, exist_ok=True)
    report = directory.parent / "product-contracts.json"
    try:
        previous = json.loads(report.read_text())
    except (OSError, ValueError):
        previous = {}
    tools = verify.tool_identity(root) if policy is not None else {}
    fingerprints = contract_fingerprints(root, policy, contracts, tools) if policy is not None else {}
    test_stamps = contract_fingerprints(root, policy, contracts, tools, "tests") if policy is not None else {}
    sites, targets = control_sites(root, policy, contracts, base) if policy is not None else ({}, set())
    required = targets if changed_only else {contract["id"] for contract in contracts}
    if in_shard is not None:
        required &= in_shard
    tests = {} if fresh else reusable_phase(previous, "tests", test_stamps)
    if not changed_only:
        # The full tier proves each test in its own process; a task's batch is not that evidence.
        tests = {name: item for name, item in tests.items() if item.get("isolation") != "batch"}
    if changed_only:
        # Passes outside this run's targets stay on record for the full tier, which rechecks each fingerprint.
        controls = {name: dict(item, reused=True) for name, item in passed_controls(previous).items()}
        reused_controls = set() if fresh else {name for name in targets if controls.get(name, {}).get("site") == sites[name]}
    else:
        controls = {} if fresh else reusable_phase(previous, "negative_controls", fingerprints)
        reused_controls = set(controls)
    reused_tests = set(tests)
    reused = reused_tests & reused_controls
    pending = [contract for contract in contracts if contract["id"] not in reused_tests]
    pending_controls = [contract for contract in contracts
                        if contract["id"] in required and contract["id"] not in reused_controls]

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
        controls[result["id"]] = dict(result, site=sites.get(result["id"]))
        execution["negative_progress"] = {
            "seconds": round(time.monotonic() - control_started, 3),
            "completed": {runner: sum(c["id"] in controls and c["id"] not in reused_controls
                                      for c in contracts if c["runner"] == runner)
                          for runner in ("rust", "swift")}}
        save("running")

    execution["identity_seconds"] = round(time.monotonic() - started, 3)
    execution["pending"] = len(contracts) - len(reused)
    execution["normal_pending"] = len(pending)
    execution["negative_pending"] = len(pending_controls)
    execution["control_scope"] = "changed" if changed_only else f"shard {shard}" if shard is not None else "all"
    execution["changed_sites"] = len(targets)
    execution["reused_tests"] = len(reused_tests)
    execution["reused_controls"] = len(reused_controls)
    save("running")
    normal_started = time.monotonic()
    run_tests(root, pending, invoke, test_stamps, directory, record_test, batched=changed_only)
    execution["normal_seconds"] = round(time.monotonic() - normal_started, 3)
    execution["normal_batched"] = sum(tests.get(c["id"], {}).get("isolation") == "batch" for c in pending)
    if pending_controls and all(tests.get(c["id"], {}).get("status") == "passed" for c in contracts):
        control_started = time.monotonic()
        execution["negative"] = negative_controls(root, pending_controls, fingerprints, directory, record_control)
        execution["negative_seconds"] = round(time.monotonic() - control_started, 3)
    if policy is not None:
        # Recheck both phases, including reused evidence, before accepting this run.
        tools = verify.tool_identity(root)
        after = contract_fingerprints(root, policy, contracts, tools)
        after_tests = contract_fingerprints(root, policy, contracts, tools, "tests")
        for entries, before, current in ((tests, test_stamps, after_tests), (controls, fingerprints, after)):
            for name in entries:
                if current.get(name) != before.get(name):
                    entries[name] = dict(entries[name], status="stale", fingerprint=None)
                    print(f"{name}: inputs changed during the run; run it again", file=sys.stderr, flush=True)
    ok = (all(tests.get(c["id"], {}).get("status") == "passed" for c in contracts)
          and all(controls.get(name, {}).get("status") == "passed" for name in required))
    if ok and changed_only:
        print(f"product contracts: {len(tests)} passed, {len(required)} negative controls with a changed site"
              f" rejected defects ({len(reused_controls)} reused); the full tier runs the rest", flush=True)
    elif ok and shard is not None:
        print(f"product contracts: {len(tests)} passed, {len(required)} negative controls of shard {shard}"
              f" rejected defects ({len(reused & required)} reused for unchanged inputs)", flush=True)
    elif ok:
        print(f"product contracts: {len(tests)} passed, {len(controls)} negative controls rejected defects"
              f" ({len(reused)} reused for unchanged inputs)", flush=True)
    save("passed" if ok else "failed")
    return 0 if ok else 1


def main():
    try:
        policy = verify.load_policy(ROOT)
        reject_unmatched_controls(ROOT, policy["product_contracts"])
        return run(ROOT, policy["product_contracts"], policy=policy,
                   fresh=os.environ.get("ECHODE_VERIFY_FORCE") == "1",
                   changed_only="--changed-controls" in sys.argv[1:], base=os.environ.get("VERIFY_BASE"),
                   shard=os.environ.get(SHARD_VARIABLE) or None)
    except (OSError, ValueError, KeyError) as error:
        report = ROOT / "target/verification/product-contracts.json"
        report.parent.mkdir(parents=True, exist_ok=True)
        try:
            previous = json.loads(report.read_text())
            if not isinstance(previous, dict):
                previous = {}
        except (OSError, ValueError):
            previous = {}
        report.write_text(json.dumps({**previous, "status": "failed", "error": str(error)}, indent=2) + "\n")
        print("product contract verification failed: " + str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
