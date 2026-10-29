#!/usr/bin/env python3
"""Print portable release-client size and digest metadata for any target binary."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import platform
import re
import shutil
import subprocess
import sys


def command_output(*command: str) -> str | None:
    if shutil.which(command[0]) is None:
        return None
    result = subprocess.run(command, check=True, capture_output=True, text=True, timeout=20)
    return result.stdout


def toolchain() -> dict[str, str] | None:
    output = command_output("rustc", "-Vv")
    if output is None:
        return None
    fields = dict(line.split(": ", 1) for line in output.splitlines() if ": " in line)
    return {key: fields[key] for key in ("release", "host", "LLVM version") if key in fields}


def sdk() -> str | None:
    if platform.system() == "Darwin":
        output = command_output("xcrun", "--show-sdk-version")
        return output.strip() if output else None
    if platform.system() == "Linux":
        output = command_output("getconf", "GNU_LIBC_VERSION")
        return output.strip() if output else None
    if platform.system() == "Windows":
        return os.environ.get("VCToolsVersion")
    return None


def native_dependencies(binary: pathlib.Path, data: bytes) -> list[str] | None:
    if data[:4] in (b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca", b"\xce\xfa\xed\xfe", b"\xcf\xfa\xed\xfe"):
        output = command_output("otool", "-L", str(binary))
        if output is None:
            return None
        lines = output.splitlines()
        return [line.strip().split(" (compatibility", 1)[0] for line in lines[1:] if line.strip()]
    if data.startswith(b"\x7fELF"):
        output = command_output("readelf", "-d", str(binary))
        return re.findall(r"Shared library: \[(.*?)\]", output) if output is not None else None
    if data.startswith(b"MZ"):
        output = command_output("dumpbin", "/dependents", str(binary)) or command_output("objdump", "-p", str(binary))
        if output is None:
            return None
        names = re.findall(r"DLL Name:\s*(\S+)", output, flags=re.IGNORECASE)
        if not names:
            names = re.findall(r"\s+([A-Za-z0-9_.-]+\.dll)\s*$", output, flags=re.IGNORECASE | re.MULTILINE)
        return sorted(set(names))
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--archive", type=pathlib.Path)
    args = parser.parse_args()
    binary = args.binary
    if not binary.is_file():
        parser.error(f"binary not found: {binary}")
    data = binary.read_bytes()
    archive_bytes = None
    archive_sha256 = None
    if args.archive is not None:
        if not args.archive.is_file():
            parser.error(f"archive not found: {args.archive}")
        archive = args.archive.read_bytes()
        archive_bytes = len(archive)
        archive_sha256 = hashlib.sha256(archive).hexdigest()
    report = {
        "target": args.target,
        "host": f"{platform.system()}-{platform.machine()}",
        "source_revision": os.environ.get("GITHUB_SHA"),
        "toolchain": toolchain(),
        "sdk_or_runtime": sdk(),
        "binary_path": str(binary),
        "raw_bytes": len(data),
        "binary_sha256": hashlib.sha256(data).hexdigest(),
        "native_dependencies": native_dependencies(binary, data),
        "archive_path": str(args.archive) if args.archive else None,
        "archive_bytes": archive_bytes,
        "archive_sha256": archive_sha256,
    }
    print(json.dumps(report, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
