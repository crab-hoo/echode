#!/usr/bin/env python3
"""Select checks from one policy and keep evidence tied to the source contents."""

import argparse
from contextlib import contextmanager
import fcntl
import fnmatch
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
import tomllib

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
POLICY = Path("scripts/verification.json")

sys.path.insert(0, str(ROOT / "scripts"))
from checked_commands import run_logged


def git(root, *arguments):
    return subprocess.check_output(["git", *arguments], cwd=root, stderr=subprocess.PIPE)


def paths(output):
    return [os.fsdecode(item) for item in output.split(b"\0") if item]


def changed_paths(root, base=None):
    # --no-renames includes both the removed path and the destination. Comparing
    # to HEAD/base includes staged and unstaged edits; untracked files need ls-files.
    reference = base or "HEAD"
    if not base and subprocess.run(
        ["git", "rev-parse", "--verify", "HEAD"], cwd=root,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    ).returncode:
        changed = paths(git(root, "ls-files", "-z", "--cached"))
    else:
        commit = git(root, "rev-parse", "--verify", "--end-of-options", f"{reference}^{{commit}}").strip().decode()
        changed = paths(git(root, "diff", "--name-only", "--no-renames", "-z", commit, "--"))
    changed += paths(git(root, "ls-files", "-z", "--others", "--exclude-standard"))
    return sorted(set(changed))


def load_policy(root):
    policy = json.loads((root / POLICY).read_text())
    if policy.get("version") != 1:
        raise ValueError("unsupported verification policy")
    seen = set()
    for check in policy["checks"]:
        name = check["id"]
        if not re.fullmatch(r"[a-z][a-z0-9-]*", name) or name in seen:
            raise ValueError("invalid/duplicate check id")
        seen.add(name)
        if check["mode"] not in ("automatic", "manual") or check["timeout"] <= 0:
            raise ValueError("invalid mode/timeout")
        if not re.fullmatch(r"[a-z][a-z0-9-]*", check.get("make_target", name)):
            raise ValueError("invalid make target")
        if not set(check["areas"]).issubset(policy["areas"]):
            raise ValueError("unknown verification area")
        for field in ("inputs", "packages", "policy_inputs", "tool_inputs"):
            values = check.get(field, [])
            if not isinstance(values, list) or not all(isinstance(value, str) and value for value in values):
                raise ValueError("invalid verification " + field)
        for value in check.get("tool_inputs", []):
            if Path(value).is_absolute() or ".." in Path(value).parts or any(c in value for c in "*?["):
                raise ValueError("verification tool inputs must be exact repository-relative paths")
        if not (check.get("inputs") or check.get("packages")):
            raise ValueError("each check needs explicit inputs or packages")
        if any(key not in policy for key in check.get("policy_inputs", [])):
            raise ValueError("unknown verification policy input")
        if check.get("extended") and check["mode"] != "manual":
            raise ValueError("extended checks must be manual")
    for group in policy.get("product_contract_inputs", {}).values():
        for field in ("inputs", "packages"):
            values = group.get(field) if isinstance(group, dict) else None
            if not isinstance(values, list) or not all(isinstance(value, str) and value for value in values):
                raise ValueError("invalid product contract " + field)
    validate_execution(policy)
    return policy


def execution_order(policy, requested):
    """Stable priority order within the requested set, respecting selected predecessor edges."""
    pending = [check for check in policy["checks"] if check["id"] in requested]
    ordered = []
    while pending:
        names = {check["id"] for check in pending}
        ready = [check for check in pending if not names.intersection(check.get("after", []))]
        if not ready:
            raise ValueError("cyclic verification execution dependencies")
        check = min(ready, key=lambda item: item.get("priority", 100))
        pending.remove(check)
        ordered.append(check)
    return ordered


def validate_execution(policy):
    checks = policy["checks"]
    names = {check["id"] for check in checks}
    preflights = policy.get("preflights", {})
    if not isinstance(preflights, dict):
        raise ValueError("invalid verification preflights")
    for name, entry in preflights.items():
        if not re.fullmatch(r"[a-z][a-z0-9-]*", name) or not isinstance(entry, dict):
            raise ValueError("invalid verification preflight")
        command = entry.get("command")
        timeout = entry.get("timeout", 0)
        if (not isinstance(command, list) or not command or not all(isinstance(arg, str) and arg for arg in command)
                or not isinstance(timeout, (int, float)) or not math.isfinite(timeout) or not 0 < timeout <= 60
                or not isinstance(entry.get("remedy"), str) or not entry["remedy"].strip()):
            raise ValueError("invalid preflight command/timeout/remedy")
        inputs = entry.get("inputs", [])
        if not isinstance(inputs, list) or not all(isinstance(value, str) and value for value in inputs):
            raise ValueError("invalid preflight inputs")
    for check in checks:
        if type(check.get("priority", 100)) is not int:
            raise ValueError("invalid verification priority")
        for field, allowed in (("after", names), ("preflight", preflights)):
            values = check.get(field, [])
            if (not isinstance(values, list) or not all(isinstance(value, str) for value in values)
                    or len(values) != len(set(values)) or not set(values).issubset(allowed)):
                raise ValueError("invalid verification " + field)
    execution_order(policy, names)


def preflight_inputs(policy, check):
    patterns = []
    for name in check.get("preflight", []):
        entry = policy["preflights"][name]
        patterns += entry.get("inputs", [])
        patterns += [arg for arg in entry["command"] if arg.startswith("scripts/")]
    return patterns


def run_preflights(root, policy, scheduled, directory):
    """No installs, builds, or device access: run each declared prerequisite once."""
    results = {}
    for check in scheduled:
        for name in check.get("preflight", []):
            if name in results:
                continue
            entry = policy["preflights"][name]
            result = run_logged(root, entry["command"], directory / f"preflight-{name}.log", entry["timeout"],
                                {**os.environ, "RUSTUP_AUTO_INSTALL": "0", "PYTHONDONTWRITEBYTECODE": "1"})
            result["remedy"] = entry["remedy"]
            results[name] = result
            if result["status"] != "passed":
                break
        if any(result["status"] != "passed" for result in results.values()):
            break
    return results


def matches(changed, patterns):
    return [path for path in changed if any(fnmatch.fnmatchcase(path, pattern) for pattern in patterns)]


RULE = re.compile(r"^([A-Za-z0-9_.-]+)[ \t]*:(.*)$")


def makefile_rules(text):
    """Split a Makefile into rule blocks and the lines that can affect every rule."""
    rules, shared, phony = {}, [], set()
    current, nested = None, 0
    for line in text.splitlines():
        stripped = line.strip()
        if nested or re.match(r"^\s*define\b", line):
            # A multi-line variable can expand into any recipe.
            nested += 1 if re.match(r"^\s*define\b", line) else -1 if stripped == "endef" else 0
            shared.append(line)
            current = None
            continue
        if line.startswith("\t") and current is not None:
            rules[current].append(line)
            continue
        if not stripped or (line.startswith("#")):
            # Make ignores comments and blank lines, including those between recipe lines.
            continue
        match = RULE.match(line)
        if match and match[1] == ".PHONY":
            phony.update(match[2].split())
            current = None
        elif match and not match[2].startswith((":", "=")) and "=" not in match[2] and match[1] not in rules:
            current = match[1]
            rules[current] = [line]
        else:
            # Variables, directives, conditionals, and unusual rules affect every check.
            shared.append(line)
            current = None
    return rules, shared, phony


def makefile_identity(parsed, target):
    rules, shared, phony = parsed
    pending, seen, missing = [target], {}, set()
    while pending:
        name = pending.pop()
        if name in seen or name in missing:
            continue
        if name not in rules:
            missing.add(name)
            continue
        seen[name] = rules[name]
        body = "\n".join(rules[name][1:])
        pending += RULE.match(rules[name][0])[2].split()
        pending += re.findall(r"\$\(MAKE\)(?:\s+-\S+)*\s+([A-Za-z0-9_.-]+)", body)
    return {"shared": shared, "rules": seen, "missing": sorted(missing), "phony": sorted(phony & set(seen))}


def definition_identities(policy, makefile):
    """The policy and Makefile text each check depends on; unrelated edits leave it unchanged."""
    parsed = makefile_rules(makefile)
    common = {key: policy.get(key) for key in ("version", "global_inputs", "infrastructure", "evidence_inputs")}
    return {check["id"]: json.dumps([common, check, {key: policy.get(key) for key in check.get("policy_inputs", [])},
                                     makefile_identity(parsed, check.get("make_target", check["id"])),
                                     {name: policy.get("preflights", {})[name] for name in check.get("preflight", [])}],
                                    sort_keys=True)
            for check in policy["checks"]}


def current_definitions(root, policy):
    makefile = root / "Makefile"
    return definition_identities(policy, makefile.read_text() if makefile.is_file() else "")


def redefined_checks(root, policy, base=None):
    """Checks whose policy entry or make recipe differs from the diff base; None when it cannot be read."""
    try:
        reference = base or "HEAD"
        previous = json.loads(git(root, "show", f"{reference}:{POLICY.as_posix()}"))
        makefile = git(root, "show", f"{reference}:Makefile").decode()
        before = definition_identities(previous, makefile)
    except (subprocess.CalledProcessError, ValueError, KeyError, TypeError, AttributeError):
        return None
    return {name for name, identity in current_definitions(root, policy).items() if before.get(name) != identity}


def select_checks(policy, changed, all_checks=False, redefined=None):
    selected = {}
    global_hits = matches(changed, policy["global_inputs"])
    # Evidence inputs are part of every check's identity, so they select every automatic check.
    infrastructure = matches(changed, policy["infrastructure"] + policy.get("evidence_inputs", []))
    # The policy and Makefile select only the checks whose definition changed.
    # Without the diff base (redefined=None), every automatic check is selected.
    definitions = matches(changed, [POLICY.as_posix(), "Makefile"])
    for check in policy["checks"]:
        hits = sorted(set(path for area in check["areas"] for path in matches(changed, policy["areas"][area])))
        reasons = hits
        if check["mode"] == "automatic":
            reasons += global_hits + infrastructure
            # Explicit inputs (including the check's own helper scripts) select it.
            reasons += matches(changed, check.get("inputs", []) + check.get("tool_inputs", [])
                               + preflight_inputs(policy, check))
            if definitions and (redefined is None or check["id"] in redefined):
                reasons += definitions
        if all_checks or (not check.get("extended") and (reasons or (changed and check.get("always")))):
            selected[check["id"]] = sorted(set(reasons)) or ["all checks" if all_checks else "every change"]
    return selected


def plan_checks(root, policy, changed, base=None, all_checks=False):
    """The selection shared by verify, doctor, and agent-context."""
    redefined = redefined_checks(root, policy, base) if matches(changed, [POLICY.as_posix(), "Makefile"]) else set()
    return select_checks(policy, changed, all_checks, redefined)


def toolchain_paths():
    # The stop hook prepends Cursor's ripgrep directory, and a login shell can
    # repeat entries. Neither changes the compiler. Record the binaries that
    # are actually resolved so an unrelated PATH prefix does not expire evidence.
    names = ("rustc", "cargo", "swift", "xcodebuild", "python3", "make", "cc", "clang")
    return {name: shutil.which(name) or "" for name in names}


def make_flags_identity(value):
    # GNU make escapes whitespace with backslashes; quotes are literal here.
    # Selection/deferral arguments cannot affect a build and must not invalidate
    # evidence when the next invocation is plain `make verify` or `make doctor`.
    # A jobserver handle names a per-invocation pipe; the -j value stays recorded.
    tokens = [re.sub(r"\\(.)", r"\1", token) for token in re.findall(r"(?:\\.|[^\s\\])+", value)]
    return [token for token in tokens if token != "--" and not token.startswith("VERIFY_ARGS=")
            and not token.startswith(("--jobserver-auth=", "--jobserver-fds="))]


def tool_identity(root):
    # Validation results must not silently survive a compiler/PATH change. Failed
    # version queries are also recorded; a selected missing tool will fail the check.
    identity = {key: os.environ.get(key, "") for key in (
        "RUSTUP_TOOLCHAIN", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS",
        "MIRIFLAGS", "ASAN_OPTIONS", "TSAN_OPTIONS", "RTSAN_ENABLE", "SDKROOT", "DEVELOPER_DIR",
        "CARGO_TARGET_DIR", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_TARGET",
        "CC", "CFLAGS", "CXX", "CXXFLAGS", "MACOSX_DEPLOYMENT_TARGET", "MAKEFLAGS",
    )}
    identity["MAKEFLAGS"] = make_flags_identity(identity["MAKEFLAGS"])
    identity["toolchain"] = toolchain_paths()
    identity["python"] = sys.version + ":" + sys.executable
    identity["system"] = list(os.uname())
    cargo_config = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")))
    for filename in ("config", "config.toml"):
        path = cargo_config / filename
        if path.is_file():
            # Config can contain private registry information; record only its hash.
            identity["cargo " + filename] = hashlib.sha256(path.read_bytes()).hexdigest()
    commands = [["rustc", "-vV"], ["cargo", "--version"], ["swift", "--version"], ["xcodebuild", "-version"]]
    env_file = root / "scripts/bridge-toolchain.env"
    if env_file.exists():
        nightly = re.search(r"^BRIDGE_NIGHTLY=([a-z0-9-]+)$", env_file.read_text(), re.MULTILINE)
        if nightly:
            commands.append(["rustc", "+" + nightly[1], "-vV"])
    for command in commands:
        try:
            result = subprocess.run(command, cwd=root, capture_output=True, timeout=15,
                                    env={**os.environ, "RUSTUP_AUTO_INSTALL": "0"})
            identity[" ".join(command)] = str(result.returncode) + ":" + os.fsdecode(result.stdout + result.stderr)
        except (OSError, subprocess.TimeoutExpired) as error:
            identity[" ".join(command)] = str(error)
    return identity


CONTENT_DIGESTS = {}


def content_digest(path):
    """Hash a regular file's mode and bytes, rehashing whenever its inode or times change.

    Every write updates ctime, so this per-process memo cannot hide an edit made
    while a check runs; a new process (the next verification) always rereads.
    """
    status = path.stat()
    key = (str(path), status.st_dev, status.st_ino, status.st_size, status.st_mtime_ns, status.st_ctime_ns,
           status.st_mode)
    if key not in CONTENT_DIGESTS:
        value = hashlib.sha256(str(status.st_mode).encode() + b"\0")
        with path.open("rb") as source:
            for block in iter(lambda: source.read(1024 * 1024), b""):
                value.update(block)
        CONTENT_DIGESTS[key] = value.digest()
    return CONTENT_DIGESTS[key]


def source_files(root):
    return sorted(set(paths(git(root, "ls-files", "-z", "--cached", "--others", "--exclude-standard"))))


def fingerprint(root, changed, base, tools):
    digest = hashlib.sha256()
    digest.update(json.dumps([changed, base, tools], sort_keys=True).encode())
    # Hash the whole non-ignored tree, including unchanged dependencies and fixtures.
    # Status/mtime alone cannot invalidate a result after another edit of the same file.
    for relative in source_files(root):
        path = root / relative
        digest.update(os.fsencode(relative) + b"\0")
        if path.is_symlink():
            digest.update(b"link:" + os.fsencode(os.readlink(path)))
        elif path.is_file():
            digest.update(content_digest(path))
        else:
            digest.update(b"missing")
        digest.update(b"\0")
    return digest.hexdigest()


def workspace_members(root):
    """Map each workspace package name to its folder."""
    workspace = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]
    manifests = {}
    for pattern in workspace["members"]:
        for folder in root.glob(pattern):
            relative = folder.relative_to(root).as_posix()
            if any(fnmatch.fnmatchcase(relative, excluded) for excluded in workspace.get("exclude", [])):
                continue
            manifest = tomllib.loads((folder / "Cargo.toml").read_text())
            manifests[manifest["package"]["name"]] = folder
    return manifests


def package_directories(root, names, kinds=("dependencies", "dev-dependencies", "build-dependencies")):
    """Include local dependencies for all features/targets, including test/build dependencies."""
    if not names:
        return set()
    workspace = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]
    manifests = workspace_members(root)
    requested = set(manifests) if names == ["*"] else set(names)
    if requested - manifests.keys():
        raise ValueError("unknown verification package: " + ", ".join(sorted(requested - manifests.keys())))
    pending = [manifests[name] for name in requested]
    visited = set()
    while pending:
        folder = pending.pop().resolve()
        if not folder.is_relative_to(root.resolve()):
            raise ValueError("verification cannot cache a local dependency outside the repository")
        if folder in visited:
            continue
        visited.add(folder)
        manifest = tomllib.loads((folder / "Cargo.toml").read_text())
        sections = [manifest, *manifest.get("target", {}).values()]
        for section in sections:
            for kind in kinds:
                for alias, dependency in section.get(kind, {}).items():
                    base = folder
                    if isinstance(dependency, dict) and dependency.get("workspace"):
                        dependency = workspace.get("dependencies", {})[alias]
                        base = root
                    if isinstance(dependency, dict) and "path" in dependency:
                        pending.append(base / dependency["path"])
    return {folder.relative_to(root.resolve()).as_posix() for folder in visited}


def audio_output_identity():
    """Read device metadata without opening an IO stream; retain only a hash."""
    try:
        completed = subprocess.run(["system_profiler", "-json", "SPAudioDataType", "-detailLevel", "mini"],
                                   capture_output=True, text=True, timeout=10, check=True)
        sections = json.loads(completed.stdout)["SPAudioDataType"]
        devices = [device for section in sections if isinstance(section, dict) for device in section.get("_items", [])
                   if isinstance(device, dict) and device.get("coreaudio_default_audio_output_device") == "spaudio_yes"]
        if len(devices) != 1:
            return None
        rate = float(devices[0].get("coreaudio_device_srate", 0))
        if not math.isfinite(rate) or rate <= 0:
            return None
        fields = ("_name", "coreaudio_device_srate", "coreaudio_device_output",
                  "coreaudio_device_manufacturer", "coreaudio_device_transport", "coreaudio_output_source")
        description = {key: devices[0].get(key) for key in fields}
        return hashlib.sha256(json.dumps(description, sort_keys=True).encode()).hexdigest()
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
        return None


def external_contexts(policy):
    return {"audio-output": audio_output_identity()} if any(c.get("device_context") for c in policy["checks"]) else {}


def input_digest(root, relative):
    path = root / relative
    value = hashlib.sha256()
    if path.is_symlink():
        value.update(b"link:" + os.fsencode(os.readlink(path)))
        path = path.resolve()
        if not path.is_relative_to(root.resolve()) or not path.is_file():
            raise ValueError("verification input symlinks must resolve to files inside the repository")
    value.update(content_digest(path) if path.is_file() else b"missing")
    return value.digest()


def scoped_digest(root, files, configuration, patterns, folders=(), tool_inputs=()):
    """Hash a configuration plus every file selected by patterns, package folders, or exact tool inputs."""
    digest = hashlib.sha256(json.dumps(configuration, sort_keys=True).encode())
    tool_inputs = set(tool_inputs)
    for relative in sorted(set(files) | tool_inputs):
        if (relative in tool_inputs or matches([relative], patterns)
                or any(relative.startswith(folder + "/") for folder in folders)):
            digest.update(os.fsencode(relative) + b"\0" + input_digest(root, relative))
    return digest.hexdigest()


def check_fingerprints(root, policy, tools, contexts=None):
    if contexts is None:
        contexts = external_contexts(policy)
    files = source_files(root)
    common = policy["global_inputs"] + policy.get("evidence_inputs", ["scripts/verify.py", ".cargo/**"])
    definitions = current_definitions(root, policy)
    stamps = {}
    for check in policy["checks"]:
        # The definition covers this check's policy entry and make recipe, not the whole Makefile.
        configuration = [definitions[check["id"]], common, tools, contexts if check.get("device_context") else {}]
        stamps[check["id"]] = scoped_digest(root, files, configuration,
                                            common + check.get("inputs", []) + preflight_inputs(policy, check),
                                            package_directories(root, check.get("packages", [])),
                                            check.get("tool_inputs", []))
    return stamps


@contextmanager
def report_lock(directory):
    directory.mkdir(parents=True, exist_ok=True)
    with (directory / ".lock").open("a") as lock:
        deadline = time.monotonic() + 30
        while True:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise ValueError("another verification run holds target/verification/.lock")
                time.sleep(0.1)
        try:
            yield
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def load_report(directory):
    try:
        report = json.loads((directory / "report.json").read_text())
        if (isinstance(report, dict) and report.get("version") == 2 and isinstance(report.get("results"), dict)
                and all(isinstance(value, dict) and isinstance(value.get("fingerprint"), str)
                        for value in report["results"].values())):
            return report
    except (OSError, ValueError):
        pass
    # Whole-tree v1 evidence cannot prove the new per-check dependency identities.
    return {"version": 2, "results": {}}


def summarize_reasons(reasons, limit=3):
    if len(reasons) <= limit:
        return ", ".join(reasons)
    return ", ".join(reasons[:limit]) + f" ほか {len(reasons) - limit} 件 (詳細は対象の変更)"


def failure_excerpt(log_path, max_lines=35, max_chars=3500):
    try:
        text = Path(log_path).read_text(errors="replace")
    except OSError:
        return ""
    lines = [line.rstrip() for line in text.splitlines()]
    pattern = re.compile(
        r"(^failures:$|^---- .* stdout ----$|^test .* \.\.\. FAILED$|^Diff in "
        r"|✘ Test .* failed|Expectation failed|^FAIL: |^ERROR: "
        r"|^error(\[[A-Z0-9]+\])?:|panicked at |SUMMARY: .*Sanitizer"
        r"|VERIFICATION TIMEOUT|^make(\[\d+\])?: \*\*\*)"
    )
    indices = [idx for idx, line in enumerate(lines) if pattern.search(line)]
    if indices:
        selected = []
        seen = set()
        for idx in indices:
            for pos in range(max(0, idx - 1), min(len(lines), idx + 8)):
                if pos not in seen:
                    seen.add(pos)
                    selected.append(lines[pos])
        excerpt = selected[-max_lines:]
    else:
        non_empty = [line for line in lines if line.strip()]
        excerpt = non_empty[-min(20, max_lines):]
    output = "\n".join(excerpt)
    if len(output) > max_chars:
        output = "...\n" + output[-max_chars:]
    return output


def save_report(directory, report, policy, selected):
    report["updated_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    temporary = directory / "report.json.tmp"
    temporary.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    temporary.replace(directory / "report.json")
    lines = ["# 変更に対する検査", "", f"更新: {report['updated_at']}",
             f"ソースの識別値: `{report['fingerprint']}`", ""]
    execution = report.get("execution", {})
    if execution:
        lines += ["## 今回の実行", "", "実行順（再利用を除く）: " + ", ".join(execution["order"]), ""]
        failure = execution.get("first_failure")
        if failure:
            lines += [f"最初の失敗: `{failure['kind']}:{failure['id']}` — 開始から {failure['seconds']} 秒", ""]
    for name, result in report.get("preflight", {}).items():
        lines.append(f"- 事前確認 `{name}`: **{result['status']}** ({result['seconds']} 秒)")
        if result["status"] != "passed":
            lines.append(f"  - 復旧: `{result['remedy']}`")
        lines.append(f"  - ログ: [preflight-{name}](preflight-{name}.log)")
    lines += ["", "## 検査結果", ""]
    for check in policy["checks"]:
        name = check["id"]
        additional = check.get("extended") and name in report["results"]
        if name not in selected and not additional:
            continue
        kind = "自動検査" if check["mode"] == "automatic" else "追加の長時間検査" if check.get("extended") else "計測などの候補"
        result = report["results"][name]
        status = result["status"]
        if result.get("fingerprint") != report["input_fingerprints"][name]:
            status = "stale"
        if name not in selected:
            kind += "・通常の完了判定外"
        lines.append(f"- `{name}` ({kind}): **{status}** — {check['purpose']}")
        if result.get("reason"):
            lines.append(f"  - 未実行の理由: {result['reason']}")
        if result.get("log"):
            lines.append(f"  - ログ: [{name}]({name}.log)")
        lines.append(f"  - 検査入力の識別値: `{result['fingerprint']}`")
        reasons = selected.get(name, ["以前に明示実行した追加検査の記録"])
        lines.append("  - 選択の根拠: " + summarize_reasons(reasons))
    lines += ["", "## 対象の変更", ""] + [f"- `{path}`" for path in report["changed_paths"]]
    lines += ["", "passed と deferred は各検査の入力・ツールが一致する間だけ有効。全体の識別値は変更監視用。not_run は実行か理由の記録が必要。deferred は合格ではない。", ""]
    temporary = directory / "report.md.tmp"
    temporary.write_text("\n".join(lines))
    temporary.replace(directory / "report.md")


def run_check(root, check, log, force=False):
    # Each recipe shell fails on an earlier command or pipeline stage, even in
    # fixture/third-party Makefiles without the repository's strict defaults.
    env = {key: value for key, value in os.environ.items() if key != "ECHODE_VERIFY_FORCE"}
    env.update({"PYTHONDONTWRITEBYTECODE": "1", "ECHODE_VERIFY_GROUP": "1", "RUSTUP_AUTO_INSTALL": "0"})
    if force:
        env["ECHODE_VERIFY_FORCE"] = "1"
    target = check.get("make_target", check["id"])
    return run_logged(root, ["make", target, "SHELL=/bin/bash -euo pipefail"],
                      log, check["timeout"], env)


def verify(root, args):
    started = time.monotonic()
    policy = load_policy(root)
    changed = changed_paths(root, args.base)
    selected = plan_checks(root, policy, changed, args.base, args.all)
    checks = {check["id"]: check for check in policy["checks"]}
    for name in args.run:
        if name in checks and checks[name].get("extended"):
            selected[name] = ["explicit --run"]
    for name in args.only + args.run + args.defer:
        if name not in selected:
            raise ValueError(f"{name} is not selected; use --all to select an otherwise unrelated check")
    for name in args.run + args.defer:
        if checks[name]["mode"] != "manual":
            raise ValueError("--run/--defer accepts only manual measurements")
    if args.defer and not (args.reason and args.reason.strip()):
        raise ValueError("--defer requires a nonempty --reason")
    if set(args.run) & set(args.defer):
        raise ValueError("cannot run and defer the same check")
    requested = set(args.only or selected)
    if args.only and not set(args.run + args.defer).issubset(requested):
        raise ValueError("--run/--defer must be included in --only")
    if args.plan:
        if changed:
            print(f"対象の変更 ({len(changed)} 件): " + summarize_reasons(changed, limit=8))
        for check in execution_order(policy, selected):
            name = check["id"]
            reasons = selected[name]
            kind = "自動検査" if checks[name]["mode"] == "automatic" else "追加の長時間検査" if checks[name].get("extended") else "計測などの候補"
            print(f"{name} [{kind}]: {checks[name]['purpose']} (根拠: {summarize_reasons(reasons)})")
        if not selected:
            print("対象の変更はありません。コミット済みの変更には --base、全検査には --all を指定します。")
        return 0
    directory = root / "target/verification"
    with report_lock(directory):
        tools = tool_identity(root)
        stamp = fingerprint(root, changed, args.base, tools)
        contexts = external_contexts(policy)
        stamps = check_fingerprints(root, policy, tools, contexts)
        report = load_report(directory)
        report.update({"fingerprint": stamp, "input_fingerprints": stamps, "changed_paths": changed,
                       "base": args.base, "selected": selected, "tools": tools})
        for name in selected:
            status = "pending" if checks[name]["mode"] == "automatic" else "not_run"
            previous = report["results"].get(name, {})
            if (previous.get("fingerprint") != stamps[name]
                    or (previous.get("status") == "passed" and checks[name].get("device_context")
                        and contexts.get("audio-output") is None)):
                report["results"][name] = {"status": status, "fingerprint": stamps[name]}
        for name in args.defer:
            report["results"][name] = {"status": "deferred", "reason": args.reason.strip(), "fingerprint": stamps[name]}
        ordered = execution_order(policy, requested)
        scheduled = [check for check in ordered
                     if (args.force or report["results"][check["id"]]["status"] != "passed")
                     and (check["mode"] == "automatic" or check["id"] in args.run)]
        report["execution"] = {"order": [check["id"] for check in scheduled], "first_failure": None}
        report["preflight"] = {}
        save_report(directory, report, policy, selected)
        report["preflight"] = run_preflights(root, policy, scheduled, directory)
        failed_preflight = next((name for name, result in report["preflight"].items()
                                 if result["status"] != "passed"), None)
        if failed_preflight:
            result = report["preflight"][failed_preflight]
            report["execution"]["first_failure"] = {
                "kind": "preflight", "id": failed_preflight, "seconds": round(time.monotonic() - started, 3)}
            save_report(directory, report, policy, selected)
            print(f"事前確認 {failed_preflight}: failed; 復旧: {result['remedy']}\n"
                  + failure_excerpt(result["log"]), file=sys.stderr)
            return 1
        save_report(directory, report, policy, selected)
        for check in ordered:
            name = check["id"]
            previous = report["results"].get(name, {}).get("status")
            if not args.force and previous == "passed":
                print(f"{name}: passed (検査の入力・ツールが同じため前回の結果を使用)", flush=True)
                continue
            if check["mode"] == "manual" and name not in args.run:
                continue
            print(f"{name}: 実行中", flush=True)
            report["results"][name] = {"status": "running", "fingerprint": stamps[name]}
            save_report(directory, report, policy, selected)
            result = run_check(root, check, directory / f"{name}.log", args.force)
            if result["status"] == "passed" and check.get("device_context") and contexts.get("audio-output") is None:
                result.update({"status": "failed", "error": "audio device metadata unavailable; cannot attest this measurement"})
            result["fingerprint"] = stamps[name]
            report["results"][name] = result
            if result["status"] != "passed":
                report["execution"]["first_failure"] = {
                    "kind": "check", "id": name, "seconds": round(time.monotonic() - started, 3)}
            # Concurrent edits may occur while an agent or CI runs a long check.
            current_tools = tool_identity(root)
            if (changed_paths(root, args.base) != changed
                    or fingerprint(root, changed, args.base, current_tools) != stamp
                    or check_fingerprints(root, policy, current_tools)[name] != stamps[name]):
                result["status"] = "stale"
                result["error"] = "source or toolchain changed during verification"
                report["execution"]["first_failure"] = {
                    "kind": "stale", "id": name, "seconds": round(time.monotonic() - started, 3)}
                save_report(directory, report, policy, selected)
                print(f"{name}: 検査中に変更がありました。再実行が必要です。", file=sys.stderr)
                return 1
            save_report(directory, report, policy, selected)
            print(f"{name}: {result['status']} ({name}.log)", flush=True)
            if result["status"] != "passed":
                excerpt = failure_excerpt(directory / f"{name}.log")
                if excerpt:
                    print(f"--- {name} 失敗箇所の抜粋 ({name}.log) ---\n{excerpt}\n---", file=sys.stderr)
                return 1
        current_tools = tool_identity(root)
        current_stamps = check_fingerprints(root, policy, current_tools)
        if (changed_paths(root, args.base) != changed or fingerprint(root, changed, args.base, current_tools) != stamp
                or any(current_stamps[name] != stamps[name] for name in selected)):
            print("検査の確認中に変更がありました。make verify を再実行してください。", file=sys.stderr)
            return 1
        # Never open hardware automatically. Every selected manual check needs
        # either explicit execution or an explanation attached to this source.
        pending = [name for name in requested
                   if report["results"][name]["status"] not in ("passed", "deferred")]
        print("結果: target/verification/report.md", flush=True)
        if pending:
            print("完了していない検査: " + ", ".join(sorted(pending)), file=sys.stderr)
            print("失敗はログを確認して修正してください。実機・計測は --run ID、未実行は --defer ID --reason 理由を使います。", file=sys.stderr)
            return 1
        return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default=os.environ.get("VERIFY_BASE"))
    parser.add_argument("--all", action="store_true")
    parser.add_argument("--only", action="append", default=[])
    parser.add_argument("--run", action="append", default=[])
    parser.add_argument("--defer", action="append", default=[])
    parser.add_argument("--reason")
    parser.add_argument("--plan", action="store_true")
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--github-matrix", action="store_true")
    args = parser.parse_args()
    try:
        if args.github_matrix:
            automatic = [check["id"] for check in load_policy(ROOT)["checks"] if check["mode"] == "automatic"]
            matrix = json.dumps(automatic)
            with Path(os.environ["GITHUB_OUTPUT"]).open("a") as output:
                output.write("checks=" + matrix + "\n")
            print(matrix)
            return 0
        return verify(ROOT, args)
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print("検査の選択または実行に失敗しました: " + str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
