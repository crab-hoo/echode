#!/usr/bin/env python3
"""Reject unsupported native-test hosts before executing M4 binaries."""

import platform
import re
import subprocess
import sys


def supports_native_tests(system, architecture, version, cpu):
    try:
        os_major = int(version.split(".")[0])
    except ValueError:
        return False
    chip = re.match(r"Apple M([0-9]+)(?:\s|$)", cpu.strip())
    return (system == "Darwin" and architecture == "arm64" and os_major >= 27
            and chip is not None and int(chip.group(1)) >= 4)


def main():
    try:
        cpu = subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError) as error:
        detail = error.stderr.strip() if isinstance(error, subprocess.CalledProcessError) and error.stderr else str(error)
        sys.exit(f"Cannot verify the M4 native-test host: {detail}. Allow the CPU information query before running native tests.")
    version = platform.mac_ver()[0]
    architecture = platform.machine()
    if not supports_native_tests(platform.system(), architecture, version, cpu):
        sys.exit(f"Native verification requires macOS 27+ on M4 or later; got {version}, {architecture}, {cpu}")
    print(f"Native verification host: macOS {version}, {cpu}, {architecture}")


if __name__ == "__main__":
    main()
