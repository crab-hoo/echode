"""Invoke the same Swift Testing helper SwiftPM uses, after a successful build."""

import os
import re
import subprocess
from functools import cache
from pathlib import Path


@cache
def runtime(root, package, product, scratch):
    # Resolve through the selected Xcode, not /usr/bin's forwarding shim. That
    # shim strips DYLD_*; the helper needs the SDK's XCTest/Testing frameworks.
    def query(args):
        return subprocess.check_output(args, cwd=root, text=True, timeout=30).strip()

    compiler = Path(query(["xcrun", "--find", "swiftc"]))
    helper = compiler.parent.parent / "libexec/swift/pm/swiftpm-testing-helper"
    platform = Path(query(["xcrun", "--show-sdk-platform-path"]))
    frameworks = platform / "Developer/Library/Frameworks"
    libraries = platform / "Developer/usr/lib"
    binary = Path(query(["swift", "build", "--package-path", package, "--scratch-path", scratch, "--show-bin-path"]))
    bundle = binary / (product + ".xctest") / "Contents/MacOS" / product
    if not helper.is_file() or not bundle.is_file() or not (frameworks / "Testing.framework").is_dir():
        raise ValueError("Swift contract runner: missing helper, test bundle, or Testing framework")
    return str(helper), str(bundle), str(frameworks), str(libraries)


def watched(root, scratch, name):
    """The event stream of one helper run, which the watchdog reads (ADR 0325)."""
    events = Path(root) / scratch / "contract-events" / (name + ".events")
    return ["--event-stream-output-path", str(events), "--event-stream-version", "0"]


def environment(frameworks, libraries):
    env = dict(os.environ)
    for name, value in (("DYLD_FRAMEWORK_PATH", frameworks), ("DYLD_LIBRARY_PATH", libraries)):
        env[name] = value + (os.pathsep + env[name] if env.get(name) else "")
    return env


def batch_invocation(root, contracts, scratch):
    """One helper run selecting every given test of one bundle, for a task's normal phase (ADR 0270)."""
    first = contracts[0]
    helper, bundle, frameworks, libraries = runtime(str(root), first["package_path"], first["build_product"], scratch)
    filters = [
        argument
        for contract in contracts
        for argument in ("--filter", re.escape(contract["suite"] + "/" + contract["test"]))
    ]
    events = watched(root, scratch, "batch-" + first["build_product"])
    return [
        helper,
        "--test-bundle-path",
        bundle,
        *filters,
        *events,
        bundle,
        "--testing-library",
        "swift-testing",
    ], environment(frameworks, libraries)


def invocation(root, contract, scratch):
    helper, bundle, frameworks, libraries = runtime(
        str(root), contract["package_path"], contract["build_product"], scratch
    )
    env = environment(frameworks, libraries)
    # The filter still must yield exactly one assertion-bearing test. The
    # evidence parser rejects zero, multiple, skipped, and known-issue results.
    selection = re.escape(contract["suite"] + "/" + contract["test"])
    return [
        helper,
        "--test-bundle-path",
        bundle,
        "--filter",
        selection,
        *watched(root, scratch, contract["id"]),
        bundle,
        "--testing-library",
        "swift-testing",
    ], env
