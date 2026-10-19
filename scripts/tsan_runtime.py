#!/usr/bin/env python3
"""Prepare a pinned, validation-only Darwin TSan with errno lifetime tracking."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
CACHE = ROOT / "target/toolchains/tsan"
UPSTREAM = CACHE / "upstream"
COMMIT = "1ab49a973e210e97d61e5db6557180dcb92c3e98"
TAG = "llvmorg-22.1.2"
PATCH = ROOT / "spikes/bridge/sanitizers/darwin-tsan.patch"
LIBRARY = "libclang_rt.tsan_osx_dynamic.dylib"
LIFETIME = "constexpr bool reset_errno_lifetime = true;"
DIRECTORIES = ("compiler-rt", "cmake", "llvm/cmake", "llvm/include")


def output(*command):
    return subprocess.check_output(command, text=True).strip()


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def context():
    return {
        "commit": COMMIT,
        "patch": sha256(PATCH),
        "builder": sha256(Path(__file__)),
        "clang": output("xcrun", "clang", "--version"),
        "sdk": output("xcrun", "--show-sdk-path"),
        "cmake": output("cmake", "--version"),
        "ninja": output("ninja", "--version"),
        "cpu": "apple-m4",
        "minimum_os": "27.0",
    }


def prepared(expected):
    try:
        manifest = json.loads((CACHE / "manifest.json").read_text())
        return manifest["context"] == expected and all(
            sha256(CACHE / variant / LIBRARY) == manifest["libraries"][variant]
            for variant in ("baseline", "fixed")
        )
    except (OSError, ValueError, KeyError):
        return False


def fetch():
    CACHE.mkdir(parents=True, exist_ok=True)
    if not UPSTREAM.exists():
        subprocess.run([
            "git", "clone", "--depth", "1", "--filter=blob:none", "--sparse",
            "--branch", TAG, "https://github.com/llvm/llvm-project.git", str(UPSTREAM),
        ], check=True)
    if output("git", "-C", str(UPSTREAM), "rev-parse", "HEAD") != COMMIT:
        raise ValueError("TSan source does not match the pinned LLVM commit")
    subprocess.run(["git", "-C", str(UPSTREAM), "sparse-checkout", "set", *DIRECTORIES], check=True)


def prepare():
    expected = context()
    if prepared(expected):
        print("TSan runtime is current")
        return
    if not UPSTREAM.exists():
        raise ValueError("TSan sources are missing; run make setup-bridge (network required)")
    if output("git", "-C", str(UPSTREAM), "rev-parse", "HEAD") != COMMIT:
        raise ValueError("TSan source does not match the pinned LLVM commit")
    source = CACHE / "source"
    # Only generated sources are replaced. The pristine, pinned checkout remains.
    if source.exists():
        shutil.rmtree(source)
    source.mkdir(parents=True)
    subprocess.run(["git", "-C", str(UPSTREAM), "diff", "--exit-code", "HEAD", "--",
                    *DIRECTORIES], check=True, stdout=subprocess.DEVNULL)
    names = subprocess.check_output(["git", "-C", str(UPSTREAM), "ls-files", "-z", "--",
                                     *DIRECTORIES]).decode().split("\0")
    for name in filter(None, names):
        destination = source / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(UPSTREAM / name, destination)
    subprocess.run(["patch", "-p1", "-i", str(PATCH)], cwd=source, check=True)
    build = CACHE / "build"
    command = [
        "cmake", "-S", str(source / "compiler-rt"), "-B", str(build), "-G", "Ninja",
        "-DCMAKE_BUILD_TYPE=Release", "-DCMAKE_OSX_ARCHITECTURES=arm64",
        "-DCMAKE_OSX_DEPLOYMENT_TARGET=27.0", "-DSANITIZER_MIN_OSX_VERSION=27.0",
        "-DCMAKE_C_COMPILER=" + output("xcrun", "--find", "clang"),
        "-DCMAKE_CXX_COMPILER=" + output("xcrun", "--find", "clang++"),
        "-DCMAKE_C_COMPILER_TARGET=arm64-apple-macos27.0",
        "-DCMAKE_CXX_COMPILER_TARGET=arm64-apple-macos27.0",
        "-DCMAKE_C_FLAGS=-mcpu=apple-m4", "-DCMAKE_CXX_FLAGS=-mcpu=apple-m4",
        "-DCOMPILER_RT_DEFAULT_TARGET_ONLY=ON", "-DCOMPILER_RT_SANITIZERS_TO_BUILD=tsan",
        "-DCOMPILER_RT_INCLUDE_TESTS=OFF",
    ]
    for component in ("BUILTINS", "XRAY", "LIBFUZZER", "PROFILE", "CTX_PROFILE", "MEMPROF", "ORC"):
        command.append(f"-DCOMPILER_RT_BUILD_{component}=OFF")
    for platform in ("IOS", "WATCHOS", "TVOS", "XROS"):
        command.append(f"-DCOMPILER_RT_ENABLE_{platform}=OFF")
    with (CACHE / "configure.log").open("w") as stream:
        subprocess.run(command, stdout=stream, stderr=subprocess.STDOUT, check=True)
    lifetime = source / "compiler-rt/lib/tsan/rtl/tsan_rtl_thread.cpp"
    fixed = lifetime.read_text()
    if fixed.count(LIFETIME) != 1:
        raise ValueError("TSan errno lifetime negative control anchor changed")
    libraries = {}
    for variant in ("baseline", "fixed"):
        lifetime.write_text(fixed.replace(LIFETIME, LIFETIME.replace("true", "false"))
                            if variant == "baseline" else fixed)
        with (CACHE / f"build-{variant}.log").open("w") as stream:
            subprocess.run(["cmake", "--build", str(build), "--target",
                            "clang_rt.tsan_osx_dynamic", "-j", "6"],
                           stdout=stream, stderr=subprocess.STDOUT, check=True)
        destination = CACHE / variant / LIBRARY
        destination.parent.mkdir(exist_ok=True)
        shutil.copy2(build / "lib/darwin" / LIBRARY, destination)
        libraries[variant] = sha256(destination)
    shutil.copy2(source / "compiler-rt/LICENSE.TXT", CACHE / "LICENSE.TXT")
    (CACHE / "manifest.json").write_text(json.dumps(
        {"context": expected, "libraries": libraries}, indent=2) + "\n")
    print("Prepared pinned Darwin TSan and its errno lifetime negative control")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fetch", action="store_true", help="fetch the pinned source and prepare")
    parser.add_argument("--prepare", action="store_true", help="build cached source without network")
    parser.add_argument("--directory", choices=("baseline", "fixed"))
    args = parser.parse_args()
    if args.fetch:
        fetch()
    if args.fetch or args.prepare:
        prepare()
    if args.directory:
        if not prepared(context()):
            raise ValueError("TSan runtime is missing or stale; run make setup-bridge")
        print(CACHE / args.directory)
    if not (args.fetch or args.prepare or args.directory):
        parser.error("select --fetch, --prepare or --directory")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit("TSan runtime: " + str(error))
