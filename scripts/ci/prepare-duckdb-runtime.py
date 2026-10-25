#!/usr/bin/env python3
"""Download and safely extract a pinned DuckDB C API test runtime."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import shutil
import sys
import tempfile
import urllib.request
import zipfile


ROOT = pathlib.Path(__file__).resolve().parents[2]
ASSETS = ROOT / "scripts/ci/duckdb-runtime-assets.json"
MAX_ARCHIVE_BYTES = 250 * 1024 * 1024
MAX_UNCOMPRESSED_BYTES = 500 * 1024 * 1024
MACHINE = {"x86_64": 0x01000007, "aarch64": 0x0100000C}


def fail(message: str) -> "NoReturn":
    raise SystemExit(f"prepare-duckdb-runtime: {message}")


def safe_member(name: str) -> pathlib.PurePosixPath:
    path = pathlib.PurePosixPath(name)
    if path.is_absolute() or not path.parts or any(part in ("", ".", "..") for part in path.parts):
        fail(f"unsafe archive member path: {name!r}")
    if "\\" in name or "\x00" in name or ":" in path.parts[0]:
        fail(f"invalid archive member path: {name!r}")
    return path


def download(url: str, destination: pathlib.Path) -> None:
    request = urllib.request.Request(url, headers={"User-Agent": "LazyDB-runtime-test-preparer"})
    with urllib.request.urlopen(request, timeout=30) as response, destination.open("wb") as output:
        total = 0
        while chunk := response.read(1024 * 1024):
            total += len(chunk)
            if total > MAX_ARCHIVE_BYTES:
                fail("download exceeds maximum archive size")
            output.write(chunk)


def file_digest(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def validate_library(path: pathlib.Path, target: str) -> None:
    with path.open("rb") as stream:
        data = stream.read(4096)
    machine = target.split("-", 1)[0]
    if "apple-darwin" in target:
        if data[:4] not in (b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca"):
            fail("macOS runtime is not a universal Mach-O library")
        endian = "big" if data[:4] == b"\xca\xfe\xba\xbe" else "little"
        count = int.from_bytes(data[4:8], endian)
        architectures = {
            int.from_bytes(data[8 + index * 20 : 12 + index * 20], endian)
            for index in range(count)
        }
        if MACHINE[machine] not in architectures:
            fail(f"macOS runtime does not contain target architecture {machine}")
    elif "linux" in target:
        expected_machine = {"x86_64": 62, "aarch64": 183}[machine]
        if data[:4] != b"\x7fELF" or data[5] != 1:
            fail("Linux runtime is not a little-endian ELF library")
        if int.from_bytes(data[18:20], "little") != expected_machine:
            fail(f"Linux runtime has the wrong architecture for {target}")
    elif "windows" in target:
        expected_machine = {"x86_64": 0x8664, "aarch64": 0xAA64}[machine]
        if data[:2] != b"MZ" or len(data) < 64:
            fail("Windows runtime is not a PE library")
        pe_offset = int.from_bytes(data[0x3C:0x40], "little")
        with path.open("rb") as stream:
            stream.seek(pe_offset)
            header = stream.read(6)
        if header[:4] != b"PE\0\0" or int.from_bytes(header[4:6], "little") != expected_machine:
            fail(f"Windows runtime has the wrong architecture for {target}")


def prepare(target: str, output_dir: pathlib.Path, cache_dir: pathlib.Path | None) -> pathlib.Path:
    metadata = json.loads(ASSETS.read_text(encoding="utf-8"))
    try:
        asset = metadata["assets"][target]
    except KeyError:
        fail(f"unsupported target: {target}")

    output_dir = output_dir.expanduser().resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    archive_hash = asset["sha256"]
    library_name = asset["library"]
    if cache_dir is not None:
        cache_dir = cache_dir.expanduser().resolve()
        cache_dir.mkdir(parents=True, exist_ok=True)
        archive = cache_dir / f"duckdb-{metadata['version']}-{target}.zip"
    else:
        archive = None

    with tempfile.TemporaryDirectory(prefix="lazydb-duckdb-runtime-") as temporary:
        temp = pathlib.Path(temporary)
        if archive is None:
            archive_path = temp / "runtime.zip"
            download(asset["url"], archive_path)
        else:
            archive_path = archive
            if not archive_path.exists() or file_digest(archive_path) != archive_hash:
                partial = cache_dir / f".{archive_path.name}.download-{os.getpid()}"
                try:
                    download(asset["url"], partial)
                    if file_digest(partial) != archive_hash:
                        fail("download SHA-256 does not match the pinned release digest")
                    os.replace(partial, archive_path)
                finally:
                    partial.unlink(missing_ok=True)

        if file_digest(archive_path) != archive_hash:
            fail("cached archive SHA-256 does not match the pinned release digest")

        selected: zipfile.ZipInfo | None = None
        seen: set[str] = set()
        total_size = 0
        with zipfile.ZipFile(archive_path) as bundle:
            for entry in bundle.infolist():
                member = safe_member(entry.filename)
                normalized = member.as_posix()
                identity = normalized.casefold()
                if identity in seen:
                    fail(f"duplicate archive path: {normalized}")
                seen.add(identity)
                mode = entry.external_attr >> 16
                if mode and (mode & 0o170000) == 0o120000:
                    fail(f"symbolic links are not accepted in runtime archives: {normalized}")
                if entry.is_dir():
                    continue
                total_size += entry.file_size
                if total_size > MAX_UNCOMPRESSED_BYTES:
                    fail("archive exceeds maximum uncompressed size")
                if member.name == library_name:
                    if selected is not None:
                        fail(f"multiple {library_name} entries in archive")
                    selected = entry
            if selected is None:
                fail(f"archive does not contain {library_name}")
            staged_library = temp / library_name
            with bundle.open(selected) as source, staged_library.open("wb") as output:
                shutil.copyfileobj(source, output)
        validate_library(staged_library, target)

        target_dir = output_dir / target
        target_dir.mkdir(parents=True, exist_ok=True)
        final_path = target_dir / library_name
        staged_output = target_dir / f".{library_name}.tmp-{os.getpid()}"
        shutil.copyfile(staged_library, staged_output)
        os.replace(staged_output, final_path)
        return final_path


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    parser.add_argument("--output-dir", type=pathlib.Path, required=True)
    parser.add_argument("--cache-dir", type=pathlib.Path)
    parser.add_argument("--print-env", action="store_true")
    args = parser.parse_args(argv)
    path = prepare(args.target, args.output_dir, args.cache_dir)
    if args.print_env:
        print(f"LAZYDB_DUCKDB_LIB_DIR={path.parent}")
    else:
        print(path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
