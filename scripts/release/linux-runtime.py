#!/usr/bin/env python3
"""Inspect Linux ELF runtime requirements and bind them to release archives."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path
from typing import Any, NoReturn


TARGETS = {
    "x86_64-unknown-linux-gnu": {
        "machine": "Advanced Micro Devices X86-64",
        "interpreter": "/lib64/ld-linux-x86-64.so.2",
    },
    "aarch64-unknown-linux-gnu": {
        "machine": "AArch64",
        "interpreter": "/lib/ld-linux-aarch64.so.1",
    },
}
SUPPORTED_TARGETS = frozenset(TARGETS)
VERSION_RE = re.compile(r"^([A-Z][A-Z0-9_]*)_(\d+(?:\.\d+)+)$")
NEED_FILE_RE = re.compile(r"\bFile:\s+(\S+)\s+Cnt:\s+(\d+)")
NEED_NAME_RE = re.compile(r"\bName:\s+(\S+)\s+Flags:\s+(\S+)\s+Version:\s+(\d+)")


class ScannerError(Exception):
    """An invalid target, ELF, or incomplete readelf report."""


def fail(message: str) -> NoReturn:
    print(f"linux runtime: {message}", file=sys.stderr)
    raise SystemExit(1)


def validate_target(target: str) -> dict[str, str]:
    try:
        return TARGETS[target]
    except KeyError as error:
        raise ScannerError(f"unsupported target: {target}") from error


def compare_versions(left: str, right: str) -> int:
    def components(value: str) -> tuple[int, ...]:
        if not re.fullmatch(r"\d+(?:\.\d+)+", value):
            raise ScannerError(f"invalid numeric ABI version: {value!r}")
        return tuple(int(part) for part in value.split("."))

    a, b = components(left), components(right)
    width = max(len(a), len(b))
    a += (0,) * (width - len(a))
    b += (0,) * (width - len(b))
    return (a > b) - (a < b)


def parse_version_requirements(output: str) -> tuple[list[dict[str, Any]], dict[int, tuple[str, str]]]:
    if "Version needs section" not in output or ".gnu.version_r" not in output:
        raise ScannerError("readelf output is missing the version needs section")
    if "contains 0 entries" in output:
        raise ScannerError("ELF has no dynamic version requirements")
    if "Version symbols section" not in output:
        raise ScannerError("readelf output is missing the version symbols section")

    definitions_match = re.search(
        r"Version definition section .*? contains (\d+) entr(?:y|ies):", output
    )
    if definitions_match is not None:
        expected_definitions = int(definitions_match.group(1))
        definition_start = definitions_match.end()
        next_section = re.search(r"\nVersion (?:symbols|needs|definition) section", output[definition_start:])
        definition_section = output[
            definition_start : definition_start + next_section.start() if next_section else None
        ]
        actual_definitions = len(
            re.findall(r"\bIndex:\s+\d+\s+Cnt:\s+\d+\s+Name:\s+\S+", definition_section)
        )
        if actual_definitions != expected_definitions:
            raise ScannerError(
                f"truncated version definitions: expected {expected_definitions}, got {actual_definitions}"
            )

    needs: list[dict[str, Any]] = []
    index_map: dict[int, tuple[str, str]] = {}
    current_library: str | None = None
    expected_count = 0
    seen_count = 0
    needs_section = output.split("Version needs section", 1)[1]
    for line in needs_section.splitlines():
        file_match = NEED_FILE_RE.search(line)
        if file_match:
            if current_library is not None and seen_count != expected_count:
                raise ScannerError(
                    f"truncated version requirements for {current_library}: "
                    f"expected {expected_count}, got {seen_count}"
                )
            current_library = file_match.group(1)
            expected_count = int(file_match.group(2))
            seen_count = 0
            continue

        name_match = NEED_NAME_RE.search(line)
        if not name_match:
            if "Version definition section" in line:
                break
            continue
        if current_library is None:
            raise ScannerError("version name appeared without a source library")
        name, flags, raw_index = name_match.groups()
        prefix_match = VERSION_RE.fullmatch(name)
        if name == "GLIBC_PRIVATE":
            raise ScannerError("unsupported private GLIBC_PRIVATE ABI requirement")
        if name.startswith("GLIBC_") and prefix_match is None:
            raise ScannerError(f"unrecognized GLIBC ABI label: {name}")
        weak = flags == "WEAK"
        if flags not in {"WEAK", "none", "BASE"}:
            raise ScannerError(f"unrecognized version requirement flags: {flags}")
        if flags == "BASE":
            raise ScannerError(f"unexpected base version in needs section: {name}")
        if current_library in {"libc.so.6", "ld-linux-x86-64.so.2", "ld-linux-aarch64.so.1"} and not name.startswith("GLIBC_"):
            raise ScannerError(f"unrecognized glibc ABI label: {name}")
        index = int(raw_index)
        needs.append({"library": current_library, "name": name, "weak": weak})
        if index in index_map:
            raise ScannerError(f"duplicate dynamic version index: {index}")
        index_map[index] = (current_library, name)
        seen_count += 1
    if current_library is None:
        raise ScannerError("version needs section contains no library entries")
    if seen_count != expected_count:
        raise ScannerError(
            f"truncated version requirements for {current_library}: "
            f"expected {expected_count}, got {seen_count}"
        )
    return needs, index_map


def parse_dynamic_symbols(
    output: str, index_map: dict[int, tuple[str, str]]
) -> list[dict[str, str]]:
    if "Symbol table '.dynsym'" not in output:
        raise ScannerError("readelf output is missing the dynamic symbol table")
    result: list[dict[str, str]] = []
    seen_symbols = 0
    for line in output.splitlines():
        if not re.match(r"^\s*\d+:\s+", line):
            continue
        seen_symbols += 1
        columns = line.split()
        if len(columns) < 8:
            continue
        binding, section, raw_name = columns[4], columns[6], columns[7]
        if binding not in {"LOCAL", "GLOBAL", "WEAK", "GNU_UNIQUE"}:
            raise ScannerError(f"unrecognized dynamic symbol binding: {binding}")
        if section != "UND" or "@" not in raw_name:
            continue
        name, version = raw_name.rsplit("@", 1)
        version = version.lstrip("@")
        version_match = VERSION_RE.fullmatch(version)
        if version.startswith("GLIBC_") and version_match is None:
            raise ScannerError(f"unrecognized GLIBC symbol ABI label: {version}")
        index_match = re.fullmatch(r"\((\d+)\)", columns[8]) if len(columns) > 8 else None
        index = int(index_match.group(1)) if index_match else None
        if index is not None and index not in index_map:
            raise ScannerError(f"symbol references unknown version index {index}: {raw_name}")
        library = index_map[index][0] if index is not None else ""
        result.append(
            {
                "name": name,
                "version": version,
                "binding": binding,
                "library": library,
            }
        )
    if seen_symbols == 0:
        raise ScannerError("readelf output contains no dynamic symbols")
    return result


def parse_dynamic(output: str) -> dict[str, Any]:
    if "Dynamic section" not in output:
        raise ScannerError("readelf output is missing the dynamic section")
    needed = re.findall(r"\(NEEDED\).*?\[([^]]+)\]", output)
    rpaths = re.findall(r"\(RPATH\).*?\[([^]]*)\]", output)
    runpaths = re.findall(r"\(RUNPATH\).*?\[([^]]*)\]", output)
    return {"needed": sorted(set(needed)), "rpath": rpaths, "runpath": runpaths}


def parse_readelf_outputs(
    target: str,
    version_info: str,
    dynamic_symbols: str,
    header: str,
    program_headers: str,
    dynamic: str,
) -> dict[str, Any]:
    expectation = validate_target(target)
    if "Class:                             ELF64" not in header and not re.search(
        r"Class:\s+ELF64", header
    ):
        raise ScannerError("expected ELF64 binary")
    if not re.search(r"Data:\s+2's complement, little endian", header):
        raise ScannerError("expected little-endian ELF")
    machine_match = re.search(r"Machine:\s*(.+)", header)
    if machine_match is None or machine_match.group(1).strip() != expectation["machine"]:
        actual = machine_match.group(1).strip() if machine_match else "missing"
        raise ScannerError(f"ELF machine mismatch for {target}: {actual}")
    interpreter_match = re.search(r"Requesting program interpreter:\s*([^]]+)", program_headers)
    if interpreter_match is None:
        raise ScannerError("ELF is missing a dynamic program interpreter")
    interpreter = interpreter_match.group(1).strip()
    if interpreter != expectation["interpreter"]:
        raise ScannerError(f"ELF interpreter mismatch for {target}: {interpreter}")

    version_needs, index_map = parse_version_requirements(version_info)
    symbols = parse_dynamic_symbols(dynamic_symbols, index_map)
    dynamic_info = parse_dynamic(dynamic)
    glibc_needs = [
        {"library": item["library"], "version": item["name"], "weak": item["weak"]}
        for item in version_needs
        if item["library"] in {"libc.so.6", "ld-linux-x86-64.so.2", "ld-linux-aarch64.so.1"}
        and item["name"].startswith("GLIBC_")
    ]
    if not glibc_needs:
        raise ScannerError("ELF has no recognizable required glibc version nodes")
    required = [item["version"][6:] for item in glibc_needs if not item["weak"]]
    if not required:
        raise ScannerError("ELF has no non-weak glibc version requirement")
    loader_min = required[0]
    for version in required[1:]:
        if compare_versions(version, loader_min) > 0:
            loader_min = version
    other_needs = [
        item for item in version_needs
        if not (
            item["library"] in {"libc.so.6", "ld-linux-x86-64.so.2", "ld-linux-aarch64.so.1"}
            and item["name"].startswith("GLIBC_")
        )
    ]
    symbol_references = [item for item in symbols if item["version"].startswith("GLIBC_")]
    return {
        "machine": expectation["machine"],
        "class": "ELF64",
        "endianness": "little",
        "interpreter": interpreter,
        **dynamic_info,
        "glibc": {
            "loader_min": loader_min,
            "version_needs": glibc_needs,
            "symbol_references": symbol_references,
        },
        "other_version_needs": other_needs,
    }


def run_readelf(binary: Path, *arguments: str) -> str:
    command = shutil.which("readelf")
    if command is None:
        raise ScannerError("GNU readelf is required to inspect Linux release binaries")
    environment = dict(os.environ)
    environment["LC_ALL"] = "C"
    try:
        result = subprocess.run(
            [command, "--wide", *arguments, str(binary)],
            check=False,
            capture_output=True,
            text=True,
            env=environment,
        )
    except OSError as error:
        raise ScannerError(f"could not run readelf: {error}") from error
    if result.returncode:
        detail = result.stderr.strip() or result.stdout.strip()
        raise ScannerError(f"readelf {' '.join(arguments)} failed: {detail}")
    return result.stdout


def inspect_elf(binary: Path, target: str) -> dict[str, Any]:
    binary = Path(binary)
    if not binary.is_file():
        raise ScannerError(f"binary is not a regular file: {binary}")
    parsed = parse_readelf_outputs(
        target,
        run_readelf(binary, "--version-info"),
        run_readelf(binary, "--dyn-syms"),
        run_readelf(binary, "--file-header"),
        run_readelf(binary, "--program-headers"),
        run_readelf(binary, "--dynamic"),
    )
    return {
        "target": target,
        "elf": {
            key: parsed[key]
            for key in ("machine", "class", "endianness", "interpreter", "needed", "rpath", "runpath")
        },
        "glibc": parsed["glibc"],
        "other_version_needs": parsed["other_version_needs"],
    }


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def archive_binary(archive: Path, version: str, target: str, output: Path) -> None:
    expected_root = f"lazydb_{version}_{target}"
    expected_binary = f"{expected_root}/lazydb"
    try:
        with tarfile.open(archive, "r:xz") as package:
            members = package.getmembers()
            names: set[str] = set()
            for member in members:
                if member.name in names:
                    raise ScannerError(f"duplicate archive member: {member.name}")
                names.add(member.name)
            matches = [item for item in members if item.name == expected_binary]
            if len(matches) != 1 or not matches[0].isreg():
                raise ScannerError(f"archive must contain one regular {expected_binary}")
            for member in members:
                if member.name.startswith("/") or any(
                    part in {"", ".", ".."} for part in Path(member.name).parts
                ) or any(character in member.name for character in "\\:\0"):
                    raise ScannerError(f"unsafe archive member: {member.name}")
                if member.name != expected_root and not member.name.startswith(expected_root + "/"):
                    raise ScannerError(f"archive member is outside expected root: {member.name}")
                if not (member.isdir() or member.isreg()):
                    raise ScannerError(f"unsupported archive member type: {member.name}")
            extracted = package.extractfile(matches[0])
            if extracted is None:
                raise ScannerError("could not read archived binary")
            with extracted, Path(output).open("wb") as destination:
                shutil.copyfileobj(extracted, destination)
    except (tarfile.TarError, OSError) as error:
        raise ScannerError(f"could not inspect release archive: {error}") from error


def build_report(version: str, target: str, archive: Path, binary: Path, elf: dict[str, Any]) -> dict[str, Any]:
    if not re.fullmatch(r"(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-beta\.[1-9]\d*)?", version):
        raise ScannerError(f"invalid release version: {version}")
    validate_target(target)
    archive_name = f"lazydb_{version}_{target}.tar.xz"
    if Path(archive).name != archive_name:
        raise ScannerError(f"archive name does not match release target: expected {archive_name}")
    return {
        "schema": 1,
        "product": "lazydb",
        "version": version,
        "target": target,
        "archive": {"name": archive_name, "sha256": sha256_file(archive)},
        "binary": {"sha256": sha256_file(binary)},
        "elf": elf["elf"],
        "glibc": elf["glibc"],
        "other_version_needs": elf["other_version_needs"],
    }


def serialize_report(report: dict[str, Any]) -> str:
    validate_report(report)
    return json.dumps(report, sort_keys=True, indent=2) + "\n"


def validate_report(report: Any) -> dict[str, Any]:
    if not isinstance(report, dict) or report.get("schema") != 1 or report.get("product") != "lazydb":
        raise ScannerError("unsupported runtime report schema or product")
    version = report.get("version")
    target = report.get("target")
    if not isinstance(version, str) or not isinstance(target, str):
        raise ScannerError("runtime report is missing version or target")
    validate_target(target)
    if not re.fullmatch(r"(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-beta\.[1-9]\d*)?", version):
        raise ScannerError("runtime report has an invalid version")
    archive = report.get("archive")
    binary = report.get("binary")
    if not isinstance(binary, dict) or not re.fullmatch(r"[0-9a-f]{64}", str(binary.get("sha256", ""))):
        raise ScannerError("runtime report has an invalid binary digest")
    if archive is not None:
        label_values = (("archive", archive),)
    else:
        label_values = ()
    for label, value in label_values:
        if not isinstance(value, dict) or not re.fullmatch(r"[0-9a-f]{64}", str(value.get("sha256", ""))):
            raise ScannerError(f"runtime report has an invalid {label} digest")
    expected_archive = f"lazydb_{version}_{target}.tar.xz"
    if archive is not None and archive.get("name") != expected_archive:
        raise ScannerError("runtime report archive name does not match version and target")
    elf = report.get("elf")
    glibc = report.get("glibc")
    if not isinstance(elf, dict) or not isinstance(glibc, dict):
        raise ScannerError("runtime report is missing ELF or glibc data")
    expectation = validate_target(target)
    if (
        elf.get("machine") != expectation["machine"]
        or elf.get("class") != "ELF64"
        or elf.get("endianness") != "little"
        or elf.get("interpreter") != expectation["interpreter"]
    ):
        raise ScannerError("runtime report ELF machine/interpreter does not match target")
    if not all(isinstance(elf.get(key), list) for key in ("needed", "rpath", "runpath")):
        raise ScannerError("runtime report dynamic ELF fields are invalid")
    minimum = glibc.get("loader_min")
    if not isinstance(minimum, str):
        raise ScannerError("runtime report is missing glibc loader minimum")
    compare_versions(minimum, minimum)
    needs = glibc.get("version_needs")
    symbols = glibc.get("symbol_references")
    other = report.get("other_version_needs")
    if not isinstance(needs, list) or not isinstance(symbols, list) or not isinstance(other, list):
        raise ScannerError("runtime report requirement collections are invalid")
    if any(
        not isinstance(item, dict)
        or not isinstance(item.get("library"), str)
        or not isinstance(item.get("version"), str)
        or not isinstance(item.get("weak"), bool)
        or not item["version"].startswith("GLIBC_")
        for item in needs
    ):
        raise ScannerError("runtime report glibc version requirement is invalid")
    if any(
        not isinstance(item, dict)
        or not isinstance(item.get("name"), str)
        or not isinstance(item.get("version"), str)
        or item.get("binding") not in {"GLOBAL", "WEAK", "GNU_UNIQUE"}
        or not isinstance(item.get("library"), str)
        for item in symbols
    ):
        raise ScannerError("runtime report glibc symbol reference is invalid")
    available_nodes = {(item["library"], item["version"]) for item in needs}
    if any((item["library"], item["version"]) not in available_nodes for item in symbols):
        raise ScannerError("runtime report symbol version is missing from its version requirements")
    if any(
        not isinstance(item, dict)
        or not isinstance(item.get("library"), str)
        or not isinstance(item.get("name"), str)
        or not isinstance(item.get("weak"), bool)
        for item in other
    ):
        raise ScannerError("runtime report non-glibc version requirement is invalid")
    if not all(isinstance(item, str) and item for item in elf["needed"] + elf["rpath"] + elf["runpath"]):
        raise ScannerError("runtime report dynamic ELF field contains an invalid value")
    if any("GLIBC_PRIVATE" in item["version"] for item in symbols + needs):
        raise ScannerError("runtime report contains unsupported GLIBC_PRIVATE ABI")
    allowed_glibc_libraries = {"libc.so.6", "ld-linux-x86-64.so.2", "ld-linux-aarch64.so.1"}
    for item in needs:
        match = VERSION_RE.fullmatch(item["version"])
        if item["library"] not in allowed_glibc_libraries or match is None or match.group(1) != "GLIBC":
            raise ScannerError("runtime report contains an unknown glibc version node/library")
    for item in symbols:
        match = VERSION_RE.fullmatch(item["version"])
        if item["library"] not in allowed_glibc_libraries or match is None or match.group(1) != "GLIBC":
            raise ScannerError("runtime report contains an unknown glibc symbol version/library")
    required = [item["version"][6:] for item in needs if not item["weak"]]
    calculated_minimum = required[0] if required else None
    for version in required[1:]:
        if compare_versions(version, calculated_minimum) > 0:
            calculated_minimum = version
    if calculated_minimum != minimum:
        raise ScannerError("runtime report glibc minimum does not match its non-weak version nodes")
    return report


def check_policy(report: dict[str, Any], policy: Any) -> None:
    if not isinstance(policy, dict) or policy.get("schema") != 1:
        raise ScannerError("unsupported Linux compatibility policy schema")
    targets = policy.get("targets")
    if not isinstance(targets, dict):
        raise ScannerError("Linux compatibility policy is missing targets")
    target_policy = targets.get(report["target"])
    if not isinstance(target_policy, dict):
        raise ScannerError(f"runtime policy has no entry for {report['target']}")
    maximum = target_policy.get("max_glibc")
    if not isinstance(maximum, str):
        raise ScannerError(f"runtime policy has no glibc maximum for {report['target']}")
    compare_versions(maximum, maximum)
    required = report["glibc"]["loader_min"]
    if compare_versions(required, maximum) > 0:
        raise ScannerError(f"glibc requirement {required} exceeds policy maximum {maximum}")
    needed = target_policy.get("needed")
    if not isinstance(needed, list) or not all(isinstance(item, str) and item for item in needed):
        raise ScannerError(f"runtime policy has an invalid NEEDED allow-list for {report['target']}")
    unexpected = sorted(set(report["elf"]["needed"]) - set(needed))
    if unexpected:
        raise ScannerError(f"ELF has libraries outside the policy allow-list: {', '.join(unexpected)}")
    if report["elf"]["interpreter"] != target_policy.get("interpreter"):
        raise ScannerError(f"ELF interpreter does not match policy for {report['target']}")
    for path in report["elf"]["rpath"] + report["elf"]["runpath"]:
        if "$ORIGIN" not in path:
            raise ScannerError(f"ELF embeds a non-relocatable RPATH/RUNPATH: {path}")
    for key in ("build_image", "build_image_digest", "validation"):
        value = target_policy.get(key)
        if not isinstance(value, str) or not value:
            raise ScannerError(f"runtime policy is missing {key} for {report['target']}")
        if value.startswith("pending-"):
            raise ScannerError(f"runtime policy {key} is still pending for {report['target']}")
    image_digest = target_policy["build_image_digest"]
    if re.fullmatch(r"sha256:[0-9a-f]{64}", image_digest) is None:
        raise ScannerError(f"runtime policy has an invalid image digest for {report['target']}")


def verify_report(report_path: Path, archive: Path, target: str, version: str) -> dict[str, Any]:
    try:
        report = json.loads(Path(report_path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ScannerError(f"could not read runtime report: {error}") from error
    validate_report(report)
    if report["target"] != target or report["version"] != version:
        raise ScannerError("runtime report version/target does not match requested release")
    if Path(archive).name != report["archive"]["name"]:
        raise ScannerError("archive name does not match runtime report")
    if sha256_file(archive) != report["archive"]["sha256"]:
        raise ScannerError("archive SHA-256 does not match runtime report")
    with tempfile.TemporaryDirectory(prefix="lazydb-runtime-verify-") as temporary:
        binary = Path(temporary) / "lazydb"
        archive_binary(archive, version, target, binary)
        if sha256_file(binary) != report["binary"]["sha256"]:
            raise ScannerError("binary SHA-256 does not match runtime report")
    return report


def verify_binary_report(report_path: Path, binary: Path, target: str, version: str) -> dict[str, Any]:
    report = validate_report(json.loads(report_path.read_text(encoding="utf-8")))
    if report["target"] != target or report["version"] != version:
        raise ScannerError("runtime report target/version does not match package inputs")
    if sha256_file(binary) != report["binary"]["sha256"]:
        raise ScannerError("package binary digest does not match runtime report")
    return report


def inspect_archive(archive: Path, version: str, target: str) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="lazydb-runtime-inspect-") as temporary:
        binary = Path(temporary) / "lazydb"
        archive_binary(archive, version, target, binary)
        elf = inspect_elf(binary, target)
        return build_report(version, target, archive, binary, elf)


def inspect_archive_with_binary(
    archive: Path, built_binary: Path, version: str, target: str
) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="lazydb-runtime-inspect-") as temporary:
        archived_binary = Path(temporary) / "lazydb"
        archive_binary(archive, version, target, archived_binary)
        if sha256_file(archived_binary) != sha256_file(built_binary):
            raise ScannerError("--binary does not match the binary inside --archive")
        elf = inspect_elf(archived_binary, target)
        return build_report(version, target, archive, archived_binary, elf)


def write_report(report: dict[str, Any], output: Path) -> None:
    output = Path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    content = serialize_report(report)
    fd, temporary_name = tempfile.mkstemp(prefix=f".{output.name}.", dir=output.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary_name, output)
    except BaseException:
        try:
            os.unlink(temporary_name)
        except FileNotFoundError:
            pass
        raise


def host_target() -> str:
    machine = os.uname().machine.lower()
    if machine in {"x86_64", "amd64"}:
        return "x86_64-unknown-linux-gnu"
    if machine in {"aarch64", "arm64"}:
        return "aarch64-unknown-linux-gnu"
    return f"{machine}-unknown-linux-gnu"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    inspect_parser = commands.add_parser("inspect", help="inspect and report an archived Linux ELF")
    inspect_parser.add_argument("--archive", type=Path, required=True)
    inspect_parser.add_argument("--binary", type=Path, required=True, help="built binary whose bytes must match the archive")
    inspect_parser.add_argument("--target", required=True, choices=sorted(TARGETS))
    inspect_parser.add_argument("--version", required=True)
    inspect_parser.add_argument("--output", type=Path, required=True)
    verify_parser = commands.add_parser("verify-report", help="verify a report against its release archive")
    verify_parser.add_argument("--report", type=Path, required=True)
    verify_parser.add_argument("--archive", type=Path, required=True)
    verify_parser.add_argument("--target", required=True, choices=sorted(TARGETS))
    verify_parser.add_argument("--version", required=True)
    binary_parser = commands.add_parser("verify-binary-report", help="verify a report against a package binary")
    binary_parser.add_argument("--report", type=Path, required=True)
    binary_parser.add_argument("--binary", type=Path, required=True)
    binary_parser.add_argument("--target", required=True, choices=sorted(TARGETS))
    binary_parser.add_argument("--version", required=True)
    policy_parser = commands.add_parser("check-policy", help="check a report against a runtime policy")
    policy_parser.add_argument("--report", type=Path, required=True)
    policy_parser.add_argument("--policy", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        if arguments.command == "inspect":
            report = inspect_archive_with_binary(
                arguments.archive, arguments.binary, arguments.version, arguments.target
            )
            write_report(report, arguments.output)
        elif arguments.command == "verify-report":
            verify_report(arguments.report, arguments.archive, arguments.target, arguments.version)
        elif arguments.command == "verify-binary-report":
            verify_binary_report(arguments.report, arguments.binary, arguments.target, arguments.version)
        else:
            report = validate_report(json.loads(arguments.report.read_text(encoding="utf-8")))
            policy = json.loads(arguments.policy.read_text(encoding="utf-8"))
            check_policy(report, policy)
    except (ScannerError, OSError, json.JSONDecodeError, tarfile.TarError) as error:
        fail(str(error))


if __name__ == "__main__":
    main()
