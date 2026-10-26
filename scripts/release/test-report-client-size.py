#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import json
import importlib.util
import pathlib
import tempfile
import unittest
from unittest import mock


SCRIPT = pathlib.Path(__file__).with_name("report-client-size.py")
SPEC = importlib.util.spec_from_file_location("report_client_size", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class ClientSizeReportTests(unittest.TestCase):
    def test_reports_target_raw_size_hash_and_archive_size(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            binary = root / "lazydb.exe"
            archive = root / "lazydb.zip"
            binary.write_bytes(b"binary")
            archive.write_bytes(b"archive")
            output = []
            with mock.patch("sys.argv", [str(SCRIPT), "--binary", str(binary), "--target", "x86_64-pc-windows-msvc", "--archive", str(archive)]), mock.patch("builtins.print", side_effect=output.append):
                self.assertEqual(MODULE.main(), 0)
            report = json.loads(output[0])
            self.assertEqual(report["raw_bytes"], len(b"binary"))
            self.assertEqual(report["binary_sha256"], hashlib.sha256(b"binary").hexdigest())
            self.assertEqual(report["archive_bytes"], len(b"archive"))
            self.assertEqual(report["archive_sha256"], hashlib.sha256(b"archive").hexdigest())
            self.assertEqual(report["target"], "x86_64-pc-windows-msvc")


if __name__ == "__main__":
    unittest.main()
