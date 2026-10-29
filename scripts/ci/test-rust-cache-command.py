#!/usr/bin/env python3
"""Verify compiler enumeration and command forwarding for rust-cache."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


WRAPPER = Path(__file__).with_name("rust-cache-command.sh")


class RustCacheCommandTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="lazydb cache command ")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        command = self.directory / "rustup"
        command.write_text('''#!/usr/bin/env python3
import json
import os
import sys
if sys.argv[1:] == ["toolchain", "list", "--quiet"]:
    print("1.94.0-x86_64-unknown-linux-gnu")
    print(os.environ["STUB_STABLE"])
else:
    print(json.dumps(sys.argv[1:]))
''')
        command.chmod(0o755)
        self.environment = dict(
            os.environ,
            PATH=str(self.directory) + os.pathsep + os.environ["PATH"],
            RUSTUP_TOOLCHAIN="1.94.0",
            STUB_STABLE="stable-x86_64-unknown-linux-gnu",
        )

    def invoke(self, *arguments, environment=None):
        return subprocess.run(
            ["sh", str(WRAPPER), *arguments],
            env=self.environment if environment is None else environment,
            capture_output=True,
            text=True,
        )

    def test_toolchain_enumeration_ignores_unused_runner_compilers(self):
        for stable in ("1.99.0-x86_64-unknown-linux-gnu", "1.99.1-x86_64-unknown-linux-gnu"):
            with self.subTest(stable=stable):
                self.environment["STUB_STABLE"] = stable
                result = self.invoke("rustup", "toolchain", "list", "--quiet")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, "1.94.0\n")

    def test_missing_toolchain_fails_instead_of_hashing_an_unpinned_compiler(self):
        environment = dict(self.environment)
        environment.pop("RUSTUP_TOOLCHAIN")
        result = self.invoke("rustup", "toolchain", "list", "--quiet", environment=environment)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        self.assertIn("RUSTUP_TOOLCHAIN", result.stderr)

    def test_compiler_probe_reaches_rustup_with_exact_arguments(self):
        result = self.invoke("rustup", "run", "1.94.0", "rustc", "-vV")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), ["run", "1.94.0", "rustc", "-vV"])

    def test_other_commands_keep_arguments_and_exit_status(self):
        result = self.invoke("sh", "-c", 'printf "%s\\n" "$1"; exit 37', "--", "two words; $(exit 1)")
        self.assertEqual(result.returncode, 37)
        self.assertEqual(result.stdout, "two words; $(exit 1)\n")


if __name__ == "__main__":
    unittest.main()
