#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import pathlib
import tempfile
import unittest
import zipfile
from unittest import mock


SCRIPT = pathlib.Path(__file__).with_name("prepare-duckdb-runtime.py")
SPEC = importlib.util.spec_from_file_location("prepare_duckdb_runtime", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class RuntimePreparerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.temporary.name)
        self.library = (
            b"\xca\xfe\xba\xbe"
            + (2).to_bytes(4, "big")
            + (0x01000007).to_bytes(4, "big")
            + bytes(16)
            + (0x0100000C).to_bytes(4, "big")
            + bytes(16)
            + bytes(2000)
        )
        self.archive = self.zip_bytes("libduckdb.dylib", self.library)
        self.asset = {
            "url": "https://example.invalid/runtime.zip",
            "sha256": hashlib.sha256(self.archive).hexdigest(),
            "library": "libduckdb.dylib",
        }

    def tearDown(self) -> None:
        self.temporary.cleanup()

    @staticmethod
    def zip_bytes(name: str, content: bytes, *, extra: tuple[str, bytes] | None = None) -> bytes:
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
            bundle.writestr(f"include/{name}", content)
            if extra is not None:
                bundle.writestr(*extra)
        return stream.getvalue()

    def run_prepare(self, archive: bytes | None = None, asset: dict | None = None) -> pathlib.Path:
        data = archive if archive is not None else self.archive
        selected = asset if asset is not None else self.asset
        if (self.root / "out").exists():
            import shutil

            shutil.rmtree(self.root / "out")
        with (
            mock.patch.object(MODULE, "ASSETS", self.root / "assets.json"),
            mock.patch.object(MODULE, "download", side_effect=lambda _url, path: path.write_bytes(data)),
        ):
            MODULE.ASSETS.write_text(
                json.dumps({"version": "1.5.6", "assets": {"x86_64-apple-darwin": selected}}),
                encoding="utf-8",
            )
            return MODULE.prepare("x86_64-apple-darwin", self.root / "out", None)

    def test_extracts_only_the_verified_runtime(self) -> None:
        result = self.run_prepare()
        self.assertEqual(result.read_bytes(), self.library)
        self.assertEqual(result.parent.name, "x86_64-apple-darwin")

    def test_rejects_digest_mismatch_without_publishing(self) -> None:
        with self.assertRaises(SystemExit):
            self.run_prepare(asset={**self.asset, "sha256": "0" * 64})
        self.assertEqual(list((self.root / "out").iterdir()), [])

    def test_rejects_path_traversal_duplicate_and_wrong_architecture(self) -> None:
        traversal = self.zip_bytes("libduckdb.dylib", self.library, extra=("../escape", b"x"))
        with self.assertRaises(SystemExit):
            self.run_prepare(traversal, {**self.asset, "sha256": hashlib.sha256(traversal).hexdigest()})

        duplicate_stream = io.BytesIO()
        with zipfile.ZipFile(duplicate_stream, "w") as bundle:
            bundle.writestr("one/libduckdb.dylib", self.library)
            bundle.writestr("two/libduckdb.dylib", self.library)
        duplicate = duplicate_stream.getvalue()
        with self.assertRaises(SystemExit):
            self.run_prepare(duplicate, {**self.asset, "sha256": hashlib.sha256(duplicate).hexdigest()})

        wrong = b"MZ" + bytes(2046)
        wrong_archive = self.zip_bytes("libduckdb.dylib", wrong)
        with self.assertRaises(SystemExit):
            self.run_prepare(wrong_archive, {**self.asset, "sha256": hashlib.sha256(wrong_archive).hexdigest()})

    def test_rejects_case_insensitive_duplicates_and_missing_library(self) -> None:
        duplicate_stream = io.BytesIO()
        with zipfile.ZipFile(duplicate_stream, "w") as bundle:
            bundle.writestr("one/libduckdb.dylib", self.library)
            bundle.writestr("one/LIBDUCKDB.DYLIB", self.library)
        duplicate = duplicate_stream.getvalue()
        with self.assertRaises(SystemExit):
            self.run_prepare(duplicate, {**self.asset, "sha256": hashlib.sha256(duplicate).hexdigest()})

        missing = self.zip_bytes("not-duckdb.dylib", self.library)
        with self.assertRaises(SystemExit):
            self.run_prepare(missing, {**self.asset, "sha256": hashlib.sha256(missing).hexdigest()})


if __name__ == "__main__":
    unittest.main()
