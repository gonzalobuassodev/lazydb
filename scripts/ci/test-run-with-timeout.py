#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import subprocess
import sys
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).with_name("run-with-timeout.py")


class TimeoutWrapperTests(unittest.TestCase):
    def invoke(self, seconds: float, *command: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPT), str(seconds), "--", *command],
            capture_output=True,
            text=True,
            timeout=10,
        )

    def test_passes_success_and_failure_exit_codes_through(self) -> None:
        self.assertEqual(self.invoke(2, sys.executable, "-c", "pass").returncode, 0)
        self.assertEqual(self.invoke(2, sys.executable, "-c", "raise SystemExit(7)").returncode, 7)

    def test_timeout_kills_a_child_process_group(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            marker = pathlib.Path(temporary) / "child-was-left-running"
            child = "import time; time.sleep(.7); open(%r, 'w').close()" % str(marker)
            parent = (
                "import subprocess,sys,time; "
                f"subprocess.Popen([sys.executable,'-c',{child!r}]); time.sleep(5)"
            )
            result = self.invoke(0.2, sys.executable, "-c", parent)
            self.assertEqual(result.returncode, 124, result.stderr)
            self.assertIn("timed out", result.stderr)
            self.assertFalse(marker.exists(), "timeout left a child running")


if __name__ == "__main__":
    unittest.main()
