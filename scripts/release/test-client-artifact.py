#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import io
import pathlib
import tarfile
import tempfile
import unittest
from unittest import mock


SCRIPT = pathlib.Path(__file__).with_name("check-client-artifact.py")
SPEC = importlib.util.spec_from_file_location("check_client_artifact", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class ClientArtifactTests(unittest.TestCase):
    def test_rejects_duckdb_c_symbols_but_allows_mangled_rust_wrappers(self) -> None:
        with mock.patch.object(MODULE, "run", return_value="0000 T _duckdb_open\n"):
            with self.assertRaisesRegex(ValueError, "DuckDB C API"):
                MODULE.inspect_symbols(pathlib.Path("lazydb"), "aarch64-apple-darwin")
        with mock.patch.object(MODULE, "run", return_value="0000 T _ZN7duckdb4open17h0123456789abcdefE\n"):
            MODULE.inspect_symbols(pathlib.Path("lazydb"), "aarch64-apple-darwin")

    def test_rejects_runtime_library_and_header_in_archive(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            archive = pathlib.Path(temporary) / "client.tar"
            with tarfile.open(archive, "w") as bundle:
                for name in ("lazydb", "libduckdb.dylib", "include/duckdb.h"):
                    info = tarfile.TarInfo(name)
                    info.size = 0
                    bundle.addfile(info, io.BytesIO())
            with self.assertRaisesRegex(ValueError, "DuckDB engine files"):
                MODULE.inspect_archive(archive)

    def test_accepts_only_client_files(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            archive = pathlib.Path(temporary) / "client.tar"
            with tarfile.open(archive, "w") as bundle:
                for name in ("lazydb", "README.md", "LICENSE-MIT", "LICENSE-APACHE"):
                    info = tarfile.TarInfo(name)
                    info.size = 0
                    bundle.addfile(info, io.BytesIO())
            MODULE.inspect_archive(archive)


if __name__ == "__main__":
    unittest.main()
