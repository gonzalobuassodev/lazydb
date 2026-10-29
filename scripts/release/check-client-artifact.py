#!/usr/bin/env python3
"""Reject DuckDB engine imports/definitions and engine files in client artifacts."""

from __future__ import annotations

import argparse
import pathlib
import re
import shutil
import subprocess
import sys
import tarfile
import zipfile


FORBIDDEN_ARCHIVE_NAMES = re.compile(r"(?:^|/)(?:libduckdb(?:\.dylib|\.so(?:\..*)?)?|duckdb\.dll|duckdb\.h|duckdb\.hpp)$", re.I)
FORBIDDEN_SYMBOL = re.compile(r"(?:^|[^A-Za-z0-9_])_?duckdb_[A-Za-z0-9_]+")


def run(*command: str) -> str:
    try:
        return subprocess.run(command, check=True, text=True, capture_output=True, timeout=30).stdout
    except (OSError, subprocess.SubprocessError) as error:
        raise ValueError(f"unable to inspect binary with {command[0]}: {error}") from error


def inspect_symbols(binary: pathlib.Path, target: str) -> None:
    if "apple-darwin" in target:
        output = run("nm", "-g", str(binary))
    elif "windows" in target:
        if shutil.which("dumpbin"):
            output = run("dumpbin", "/symbols", str(binary))
        elif shutil.which("llvm-nm"):
            output = run("llvm-nm", "-g", str(binary))
        elif shutil.which("objdump"):
            output = run("objdump", "-p", str(binary))
        else:
            raise ValueError("Windows artifact inspection requires dumpbin, llvm-nm, or objdump")
    else:
        output = run("nm", "-g", str(binary))
    matches = sorted(set(FORBIDDEN_SYMBOL.findall(output)))
    if matches:
        raise ValueError("client binary contains DuckDB C API imports or definitions: " + ", ".join(matches[:12]))


def inspect_archive(archive: pathlib.Path) -> None:
    names: list[str]
    if zipfile.is_zipfile(archive):
        with zipfile.ZipFile(archive) as bundle:
            names = bundle.namelist()
    elif tarfile.is_tarfile(archive):
        with tarfile.open(archive, "r:*") as bundle:
            names = bundle.getnames()
    else:
        raise ValueError(f"unsupported release archive format: {archive}")
    forbidden = [name for name in names if FORBIDDEN_ARCHIVE_NAMES.search(name)]
    if forbidden:
        raise ValueError("client archive contains DuckDB engine files: " + ", ".join(forbidden))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--archive", type=pathlib.Path, required=True)
    parser.add_argument("--target", required=True)
    args = parser.parse_args(argv)
    try:
        inspect_symbols(args.binary, args.target)
        inspect_archive(args.archive)
    except ValueError as error:
        print(f"client artifact check failed: {error}", file=sys.stderr)
        return 1
    print(f"client artifact excludes DuckDB engine imports and payload: {args.target}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
