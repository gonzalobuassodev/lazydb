#!/usr/bin/env python3
"""Contract and Linux ELF integration tests for the runtime scanner."""

from __future__ import annotations

import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
import unittest.mock
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCANNER = ROOT / "scripts/release/linux-runtime.py"
FIXTURES = ROOT / "scripts/release/fixtures/linux-runtime"


def load_scanner():
    spec = importlib.util.spec_from_file_location("linux_runtime", SCANNER)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not load scanner: {SCANNER}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@unittest.skipUnless(sys.platform.startswith("linux"), "real ELF integration requires Linux")
class LinuxElfIntegrationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.cc = shutil.which(os.environ.get("CC", "cc"))
        if cls.cc is None:
            raise RuntimeError("Linux runtime scanner integration requires a C compiler")
        cls.scanner = load_scanner()

    def test_scans_real_elf_and_retains_dynamic_metadata_after_strip(self):
        target = self.scanner.host_target()
        if target not in self.scanner.SUPPORTED_TARGETS:
            self.fail(f"unsupported native test host: {target}")

        with tempfile.TemporaryDirectory(prefix="lazydb-runtime-test-") as temporary:
            binary = Path(temporary) / "hello"
            source = FIXTURES / "hello.c"
            subprocess.run([self.cc, "-Wl,--build-id=none", str(source), "-o", str(binary)], check=True)
            report = self.scanner.inspect_elf(binary, target)
            self.assertEqual(report["elf"]["machine"], self.scanner.TARGETS[target]["machine"])
            self.assertTrue(report["elf"]["interpreter"])
            self.assertTrue(report["elf"]["needed"])
            self.assertTrue(report["glibc"]["version_needs"])

            strip = shutil.which("strip")
            if strip is None:
                self.fail("strip unavailable after unstripped ELF inspection")
            subprocess.run([strip, "--strip-all", str(binary)], check=True)
            stripped = self.scanner.inspect_elf(binary, target)
            self.assertEqual(stripped["glibc"]["version_needs"], report["glibc"]["version_needs"])
            archive = Path(temporary) / "lazydb_0.0.0_x86_64-unknown-linux-gnu.tar.xz"
            root = Path(temporary) / "lazydb_0.0.0_x86_64-unknown-linux-gnu"
            root.mkdir()
            archived_binary = root / "lazydb"
            shutil.copyfile(binary, archived_binary)
            with tarfile.open(archive, "w:xz") as package:
                package.add(root, arcname=root.name)
            runtime_report = self.scanner.build_report(
                "0.0.0", target, archive, archived_binary, self.scanner.inspect_elf(archived_binary, target)
            )
            report_path = Path(temporary) / "runtime.json"
            report_path.write_text(self.scanner.serialize_report(runtime_report))
            verified = self.scanner.verify_report(report_path, archive, target, "0.0.0")
            self.assertEqual(verified["binary"]["sha256"], self.scanner.sha256_file(archived_binary))
            subprocess.run([str(binary)], check=True, capture_output=True, text=True)


class RuntimeScannerContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.scanner = load_scanner()

    def parse(self, target):
        return self.scanner.parse_readelf_outputs(
            target,
            (FIXTURES / f"{target.split('-')[0]}-version-info.txt").read_text(),
            (FIXTURES / f"{target.split('-')[0]}-dynamic-symbols.txt").read_text(),
            "ELF Header:\n  Class: ELF64\n  Data: 2's complement, little endian\n  Machine: "
            + ("Advanced Micro Devices X86-64" if target.startswith("x86_64") else "AArch64")
            + "\n",
            "Program Headers:\n  INTERP 0x0 0x0 0x0 0x1 0x1 R 0x1\n      [Requesting program interpreter: "
            + ("/lib64/ld-linux-x86-64.so.2" if target.startswith("x86_64") else "/lib/ld-linux-aarch64.so.1")
            + "]\n",
            "Dynamic section at offset 0x0 contains 4 entries:\n"
            " 0x0000000000000001 (NEEDED)             Shared library: [libc.so.6]\n"
            + (" 0x0000000000000001 (NEEDED)             Shared library: [libm.so.6]\n" if target.startswith("x86_64") else ""),
        )

    def test_compares_glibc_versions_numerically(self):
        compare = self.scanner.compare_versions
        self.assertLess(compare("2.9", "2.38"), 0)
        self.assertEqual(compare("2.38", "2.38.0"), 0)
        self.assertGreater(compare("2.40", "2.39"), 0)

    def test_weak_symbol_does_not_lower_nonweak_glibc_version_need(self):
        report = self.parse("x86_64-unknown-linux-gnu")
        self.assertEqual(report["glibc"]["loader_min"], "2.34")
        self.assertIn(
            {"library": "libc.so.6", "version": "GLIBC_2.39", "weak": True},
            report["glibc"]["version_needs"],
        )
        pidfd = next(
            item for item in report["glibc"]["symbol_references"] if item["name"] == "pidfd_spawnp"
        )
        self.assertEqual(pidfd["binding"], "WEAK")

    def test_aarch64_tracks_libc_and_other_library_versions_separately(self):
        report = self.parse("aarch64-unknown-linux-gnu")
        self.assertEqual(report["glibc"]["loader_min"], "2.17")
        self.assertEqual(report["glibc"]["version_needs"][0]["library"], "libc.so.6")
        self.assertEqual(
            report["other_version_needs"],
            [{"library": "libgcc_s.so.1", "name": "GCC_4.2.0", "weak": False}],
        )

    def test_version_needs_without_version_definitions_are_valid(self):
        fixture = (FIXTURES / "aarch64-version-info.txt").read_text()
        no_definitions = fixture.split("Version definition section", 1)[0]
        needs, index_map = self.scanner.parse_version_requirements(no_definitions)
        self.assertEqual(len(needs), 3)
        self.assertEqual(index_map[4], ("libc.so.6", "GLIBC_2.39"))

    def test_policy_rejects_glibc_and_needed_library_drift(self):
        target = "x86_64-unknown-linux-gnu"
        parsed = self.parse(target)
        elf = {
            "elf": {
                key: parsed[key]
                for key in ("machine", "class", "endianness", "interpreter", "needed", "rpath", "runpath")
            },
            "glibc": parsed["glibc"],
            "other_version_needs": parsed["other_version_needs"],
        }
        with tempfile.TemporaryDirectory(prefix="lazydb-runtime-policy-") as temporary:
            directory = Path(temporary)
            archive = directory / f"lazydb_1.2.3_{target}.tar.xz"
            archive.write_bytes(b"archive")
            binary = directory / "lazydb"
            binary.write_bytes(b"binary")
            report = self.scanner.build_report("1.2.3", target, archive, binary, elf)
            policy = {
                "schema": 1,
                "targets": {
                    target: {
                        "max_glibc": "2.34",
                        "needed": ["libc.so.6", "libm.so.6"],
                        "interpreter": "/lib64/ld-linux-x86-64.so.2",
                        "build_image": "ubuntu:24.04",
                        "build_image_digest": "sha256:" + "0" * 64,
                        "validation": "native-linux-ci",
                    }
                },
            }
            self.scanner.check_policy(report, policy)
            too_old = json.loads(json.dumps(policy))
            too_old["targets"][target]["max_glibc"] = "2.33"
            with self.assertRaisesRegex(self.scanner.ScannerError, "exceeds policy"):
                self.scanner.check_policy(report, too_old)
            drift = json.loads(json.dumps(policy))
            drift["targets"][target]["needed"].remove("libm.so.6")
            with self.assertRaisesRegex(self.scanner.ScannerError, "allow-list"):
                self.scanner.check_policy(report, drift)
            pending = json.loads(json.dumps(policy))
            pending["targets"][target]["build_image_digest"] = "pending-evidence"
            with self.assertRaisesRegex(self.scanner.ScannerError, "still pending"):
                self.scanner.check_policy(report, pending)

    def test_policy_rejects_rpath_even_if_library_allowlist_matches(self):
        target = "x86_64-unknown-linux-gnu"
        parsed = self.parse(target)
        parsed["rpath"] = ["/opt/build/lib"]
        elf = {
            "elf": {
                key: parsed[key]
                for key in ("machine", "class", "endianness", "interpreter", "needed", "rpath", "runpath")
            },
            "glibc": parsed["glibc"],
            "other_version_needs": parsed["other_version_needs"],
        }
        with tempfile.TemporaryDirectory(prefix="lazydb-runtime-rpath-policy-") as temporary:
            directory = Path(temporary)
            archive = directory / f"lazydb_1.2.3_{target}.tar.xz"
            binary = directory / "lazydb"
            archive.write_bytes(b"archive")
            binary.write_bytes(b"binary")
            report = self.scanner.build_report("1.2.3", target, archive, binary, elf)
            policy = json.loads((ROOT / "scripts/release/linux-compatibility-policy.json").read_text())
            policy["targets"][target]["needed"] = ["libc.so.6", "libm.so.6"]
            with self.assertRaisesRegex(self.scanner.ScannerError, "RPATH"):
                self.scanner.check_policy(report, policy)

    def test_policy_values_are_per_arch_and_reference_native_release_evidence(self):
        policy = json.loads((ROOT / "scripts/release/linux-compatibility-policy.json").read_text())
        x64 = policy["targets"]["x86_64-unknown-linux-gnu"]
        arm = policy["targets"]["aarch64-unknown-linux-gnu"]
        self.assertNotEqual(x64["build_image_digest"], arm["build_image_digest"])
        self.assertTrue(x64["build_image_digest"].startswith("sha256:"))
        self.assertTrue(arm["build_image_digest"].startswith("sha256:"))
        self.assertEqual(x64["validation"], "release-workflow-native-build-and-smoke")
        self.assertEqual(arm["validation"], "release-workflow-native-build-and-smoke")

    def test_build_wrapper_requires_native_architecture_and_exact_image_digest(self):
        script = ROOT / "scripts/release/build-linux.sh"
        environment = dict(os.environ)
        environment["LAZYDB_LINUX_BUILD_IMAGE"] = (
            "ubuntu:24.04@sha256:f610ab94648195aa356059f5b41d6085c9d4d903c072430cdd1af7bdb646106b"
        )
        environment["PATH"] = "/usr/bin:/bin"
        result = subprocess.run(
            ["sh", str(script), "aarch64-unknown-linux-gnu"],
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("native runner required", result.stderr)

    def test_checked_in_policy_has_no_pending_fields(self):
        policy = json.loads((ROOT / "scripts/release/linux-compatibility-policy.json").read_text())
        for target in self.scanner.SUPPORTED_TARGETS:
            entry = policy["targets"][target]
            for key in ("build_image_digest", "validation"):
                self.assertFalse(entry[key].startswith("pending-"), f"{target} {key} is pending")

    def test_rejects_unknown_target_and_private_glibc_abi(self):
        with self.assertRaisesRegex(self.scanner.ScannerError, "unsupported target"):
            self.scanner.validate_target("mips-unknown-linux-gnu")
        malformed = (FIXTURES / "corrupt-version-info.txt").read_text()
        with self.assertRaisesRegex(self.scanner.ScannerError, "GLIBC_PRIVATE"):
            self.scanner.parse_version_requirements(malformed)

    def test_rejects_unrecognized_glibc_versions_even_when_numeric_requirement_exists(self):
        fixture = (FIXTURES / "x86_64-version-info.txt").read_text()
        with self.assertRaisesRegex(self.scanner.ScannerError, "unrecognized GLIBC ABI"):
            self.scanner.parse_version_requirements(fixture.replace("Name: GLIBC_2.39", "Name: GLIBC_future"))
        with self.assertRaisesRegex(self.scanner.ScannerError, "unexpected base version"):
            self.scanner.parse_version_requirements(fixture.replace("Flags: WEAK", "Flags: BASE"))

    def test_rejects_truncated_readelf_output(self):
        with self.assertRaises(self.scanner.ScannerError):
            self.scanner.parse_version_requirements("Version needs section '.gnu.version_r'")
        fixture = (FIXTURES / "x86_64-version-info.txt").read_text()
        with self.assertRaisesRegex(self.scanner.ScannerError, "truncated version definitions"):
            self.scanner.parse_version_requirements(
                fixture.replace(
                    "  0x0060: Rev: 1  Flags: none  Index: 4  Cnt: 1  Name: APP_1.0\n", ""
                )
            )

    def test_report_is_deterministic_and_hashes_the_archive_binary(self):
        with tempfile.TemporaryDirectory(prefix="lazydb-runtime-contract-") as temporary:
            directory = Path(temporary)
            target = "x86_64-unknown-linux-gnu"
            archive = directory / "lazydb_1.2.3_x86_64-unknown-linux-gnu.tar.xz"
            archive.write_bytes(b"fixture archive")
            package = directory / "lazydb_1.2.3_x86_64-unknown-linux-gnu"
            package.mkdir()
            binary = package / "lazydb"
            binary.write_bytes(b"fixture binary")
            parsed = self.parse(target)
            elf = {
                "elf": {key: parsed[key] for key in ("machine", "class", "endianness", "interpreter", "needed", "rpath", "runpath")},
                "glibc": parsed["glibc"],
                "other_version_needs": parsed["other_version_needs"],
            }
            report = self.scanner.build_report(
                version="1.2.3", target=target, archive=archive, binary=binary, elf=elf,
            )
            first = self.scanner.serialize_report(report)
            second = self.scanner.serialize_report(report)
            self.assertEqual(first, second)
            self.assertTrue(first.endswith("\n"))
            data = json.loads(first)
            self.assertEqual(data["archive"]["name"], archive.name)
            self.assertEqual(data["binary"]["sha256"], self.scanner.sha256_file(binary))
            self.assertEqual(data["glibc"]["loader_min"], "2.34")
            self.scanner.validate_report(data)
            data["glibc"]["loader_min"] = "2.99"
            with self.assertRaisesRegex(self.scanner.ScannerError, "does not match"):
                self.scanner.validate_report(data)
            for malformed in (
                "GLIBC_bad",
                "GLIBC_PRIVATE",
            ):
                data = json.loads(first)
                data["glibc"]["version_needs"][0]["version"] = malformed
                with self.assertRaises(self.scanner.ScannerError):
                    self.scanner.validate_report(data)

    def test_symbol_reference_must_resolve_to_the_reported_requirement(self):
        parsed = self.parse("x86_64-unknown-linux-gnu")
        elf = {
            "elf": {
                key: parsed[key]
                for key in ("machine", "class", "endianness", "interpreter", "needed", "rpath", "runpath")
            },
            "glibc": parsed["glibc"],
            "other_version_needs": parsed["other_version_needs"],
        }
        elf["glibc"]["symbol_references"].append(
            {"name": "fake", "version": "GLIBC_2.99", "binding": "GLOBAL", "library": "libc.so.6"}
        )
        with tempfile.TemporaryDirectory(prefix="lazydb-runtime-invalid-report-") as temporary:
            directory = Path(temporary)
            archive = directory / "lazydb_1.2.3_x86_64-unknown-linux-gnu.tar.xz"
            archive.write_bytes(b"fixture archive")
            binary = directory / "lazydb"
            binary.write_bytes(b"fixture binary")
            report = self.scanner.build_report("1.2.3", "x86_64-unknown-linux-gnu", archive, binary, elf)
            with self.assertRaisesRegex(self.scanner.ScannerError, "symbol version"):
                self.scanner.validate_report(report)

    def test_real_archive_cli_rejects_mismatched_binary_before_readelf(self):
        with tempfile.TemporaryDirectory(prefix="lazydb-runtime-cli-mismatch-") as temporary:
            directory = Path(temporary)
            root_name = "lazydb_1.2.3_x86_64-unknown-linux-gnu"
            root = directory / root_name
            root.mkdir()
            archived_binary = root / "lazydb"
            archived_binary.write_bytes(b"archived")
            archive = directory / f"{root_name}.tar.xz"
            with tarfile.open(archive, "w:xz") as package:
                package.add(root, arcname=root_name)
            mismatch = directory / "mismatch"
            mismatch.write_bytes(b"different")
            output = directory / "report.json"
            result = subprocess.run(
                [
                    sys.executable, str(SCANNER), "inspect", "--archive", str(archive),
                    "--binary", str(mismatch), "--target", "x86_64-unknown-linux-gnu",
                    "--version", "1.2.3", "--output", str(output),
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("does not match", result.stderr)
            self.assertFalse(output.exists())

    def test_archive_verification_binds_report_to_real_contents(self):
        with tempfile.TemporaryDirectory(prefix="lazydb-runtime-archive-") as temporary:
            directory = Path(temporary)
            target = "x86_64-unknown-linux-gnu"
            root_name = "lazydb_1.2.3_x86_64-unknown-linux-gnu"
            root = directory / root_name
            root.mkdir()
            binary = root / "lazydb"
            binary.write_bytes(b"fixture binary")
            (root / "README.md").write_text("fixture\n")
            archive = directory / f"{root_name}.tar.xz"
            import tarfile

            with tarfile.open(archive, "w:xz") as package:
                package.add(root, arcname=root_name)
            parsed = self.parse(target)
            elf = {
                "elf": {
                    key: parsed[key]
                    for key in ("machine", "class", "endianness", "interpreter", "needed", "rpath", "runpath")
                },
                "glibc": parsed["glibc"],
                "other_version_needs": parsed["other_version_needs"],
            }
            report = self.scanner.build_report("1.2.3", target, archive, binary, elf)
            report_path = directory / "runtime.json"
            report_path.write_text(self.scanner.serialize_report(report))
            self.scanner.verify_report(report_path, archive, target, "1.2.3")
            binary.write_bytes(b"tampered binary")
            with tarfile.open(archive, "w:xz") as package:
                package.add(root, arcname=root_name)
            with self.assertRaisesRegex(self.scanner.ScannerError, "archive SHA-256"):
                self.scanner.verify_report(report_path, archive, target, "1.2.3")

    def test_package_binary_report_verifies_digest_and_identity(self):
        with tempfile.TemporaryDirectory(prefix="lazydb-runtime-package-report-") as temporary:
            directory = Path(temporary)
            target = "x86_64-unknown-linux-gnu"
            binary = directory / "lazydb"
            binary.write_bytes(b"package binary")
            archive = directory / f"lazydb_1.2.3_{target}.tar.xz"
            archive.write_bytes(b"archive")
            parsed = self.parse(target)
            elf = {"elf": {key: parsed[key] for key in ("machine", "class", "endianness", "interpreter", "needed", "rpath", "runpath")}, "glibc": parsed["glibc"], "other_version_needs": parsed["other_version_needs"]}
            report = self.scanner.build_report("1.2.3", target, archive, binary, elf)
            report_path = directory / "runtime.json"
            report_path.write_text(self.scanner.serialize_report(report))
            self.scanner.verify_binary_report(report_path, binary, target, "1.2.3")
            corrupted = directory / "corrupted"
            corrupted.write_bytes(b"different")
            with self.assertRaisesRegex(self.scanner.ScannerError, "digest does not match"):
                self.scanner.verify_binary_report(report_path, corrupted, target, "1.2.3")

            binary_only = dict(report)
            binary_only.pop("archive")
            bare_path = directory / "binary-only.json"
            bare_path.write_text(json.dumps(binary_only))
            self.scanner.verify_binary_report(bare_path, binary, target, "1.2.3")

    def test_archive_rejects_duplicate_and_unsafe_members(self):
        with tempfile.TemporaryDirectory(prefix="lazydb-runtime-tar-") as temporary:
            directory = Path(temporary)
            root_name = "lazydb_1.2.3_x86_64-unknown-linux-gnu"
            archive = directory / f"{root_name}.tar.xz"
            import io
            import tarfile

            with tarfile.open(archive, "w:xz") as package:
                for name in (f"{root_name}/lazydb", f"{root_name}/lazydb"):
                    info = tarfile.TarInfo(name)
                    info.size = 1
                    package.addfile(info, io.BytesIO(b"x"))
            with self.assertRaisesRegex(self.scanner.ScannerError, "duplicate"):
                self.scanner.archive_binary(archive, "1.2.3", "x86_64-unknown-linux-gnu", directory / "out")

            with tarfile.open(archive, "w:xz") as package:
                info = tarfile.TarInfo(f"{root_name}/../escape")
                info.size = 1
                package.addfile(info, io.BytesIO(b"x"))
                info = tarfile.TarInfo(f"{root_name}/lazydb")
                info.size = 1
                package.addfile(info, io.BytesIO(b"x"))
            with self.assertRaisesRegex(self.scanner.ScannerError, "unsafe"):
                self.scanner.archive_binary(archive, "1.2.3", "x86_64-unknown-linux-gnu", directory / "out")


if __name__ == "__main__":
    unittest.main(verbosity=2)
