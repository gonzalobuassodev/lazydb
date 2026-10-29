#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import pathlib
import unittest


SCRIPT = pathlib.Path(__file__).with_name("check-update-dependencies.py")
SPEC = importlib.util.spec_from_file_location("check_update_dependencies", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class TargetDependencyTests(unittest.TestCase):
    def test_accepts_target_specific_decoder_sets(self) -> None:
        MODULE.check("x86_64-pc-windows-msvc", "zip v6.0.0\nflate2 v1.1.9\n")
        MODULE.check("aarch64-apple-darwin", "tar v0.4.46\nxz2 v0.1.7\n")

    def test_rejects_cross_platform_and_extra_decoder_dependencies(self) -> None:
        with self.assertRaises(ValueError):
            MODULE.check("x86_64-pc-windows-msvc", "zip v6.0.0\ntar v0.4.46\nxz2 v0.1.7")
        with self.assertRaises(ValueError):
            MODULE.check("aarch64-apple-darwin", "tar v0.4.46\nxz2 v0.1.7\nzip v6.0.0")
        with self.assertRaises(ValueError):
            MODULE.check("x86_64-pc-windows-msvc", "zip v6.0.0\nzopfli v0.8.3")


if __name__ == "__main__":
    unittest.main()
