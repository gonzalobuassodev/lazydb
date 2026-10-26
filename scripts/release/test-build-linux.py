#!/usr/bin/env python3
"""Exercise the Linux build wrapper without Docker or a Rust compilation."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
IMAGES = {
    "x86_64": "ubuntu:22.04@sha256:98510ac07bb4545e389be502a8918b385c018bfe4ceeb5ac7e0375c782268b04",
    "aarch64": "ubuntu:22.04@sha256:88881eb36da489f9ff8746ece8048ca16cc94f80e8cd3ae7e8335d8822631591",
}

DOCKER_STUB = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import subprocess
import sys

args = sys.argv[1:]
with open(os.environ["STUB_DOCKER_LOG"], "a") as log:
    log.write(json.dumps(args) + "\n")
if args[0] == "build":
    print("Preparing build image")
    sys.exit(int(os.environ.get("STUB_BUILD_STATUS", "0")))
assert args[0] == "run", args
status = int(os.environ.get("STUB_RUN_STATUS", "0"))
if status:
    sys.exit(status)
image_index = next(i for i, arg in enumerate(args) if arg.startswith("lazydb-linux-build:"))
options = args[:image_index]
environment = dict(os.environ)
for index, option in enumerate(options):
    if option == "--env":
        key, value = options[index + 1].split("=", 1)
        environment[key] = value
cwd = options[options.index("--workdir") + 1]
assert options[options.index("--entrypoint") + 1] == "/bin/bash"
sys.exit(subprocess.run(["/bin/bash", *args[image_index + 1:]], cwd=cwd, env=environment).returncode)
'''

CARGO_STUB = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys

args = sys.argv[1:]
assert os.environ["RUSTUP_TOOLCHAIN"] == "1.94.0"
if "metadata" in args:
    print(json.dumps({
        "workspace_root": str(Path.cwd()),
        "target_directory": str(Path.cwd() / "target"),
        "packages": [{"manifest_path": str(Path(os.environ["CARGO_HOME"]) / "registry/src/dependency/Cargo.toml")}],
    }))
elif "build" in args:
    assert args == ["+1.94.0", "build", "--release", "--locked", "--target", args[5], "--timings"], args
    if os.environ.get("STUB_SKIP_BINARY") != "1":
        binary = Path("target") / args[args.index("--target") + 1] / "release/lazydb"
        binary.parent.mkdir(parents=True, exist_ok=True)
        binary.write_text("fake binary\n")
else:
    sys.exit("unexpected Cargo arguments: " + repr(args))
'''


class LinuxBuildWrapperTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="lazydb build wrapper ")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name).resolve()
        self.repo = self.directory / "repository with spaces"
        self.script = self.repo / "scripts/release/build-linux.sh"
        self.script.parent.mkdir(parents=True)
        shutil.copyfile(ROOT / "scripts/release/build-linux.sh", self.script)
        shutil.copyfile(ROOT / "scripts/release/Dockerfile.linux", self.script.parent / "Dockerfile.linux")
        (self.repo / "scripts/ci").mkdir(parents=True)
        shutil.copyfile(ROOT / "scripts/ci/run-with-timeout.py", self.repo / "scripts/ci/run-with-timeout.py")
        self.bin = self.directory / "bin"
        self.bin.mkdir()
        for name, contents in (
            ("docker", DOCKER_STUB),
            ("cargo", CARGO_STUB),
            ("uname", '#!/bin/sh\nprintf \'%s\\n\' "$STUB_ARCH"\n'),
        ):
            executable = self.bin / name
            executable.write_text(contents)
            executable.chmod(0o755)
        self.cargo_home = self.directory / "cargo home"
        self.log = self.directory / "docker.jsonl"
        self.target = "x86_64-unknown-linux-gnu"
        self.environment = dict(
            os.environ,
            PATH=str(self.bin) + os.pathsep + os.environ["PATH"],
            CARGO_HOME=str(self.cargo_home),
            CARGO_BUILD_JOBS="2",
            CARGO_INCREMENTAL="0",
            RUSTUP_TOOLCHAIN="stable",
            STUB_ARCH="x86_64",
            STUB_DOCKER_LOG=str(self.log),
            LAZYDB_LINUX_BUILD_IMAGE=IMAGES["x86_64"],
        )

    def invoke(self, *arguments, **environment):
        self.log.unlink(missing_ok=True)
        return subprocess.run(
            ["sh", str(self.script), *arguments],
            cwd=self.directory,
            env=dict(self.environment, **environment),
            capture_output=True,
            text=True,
        )

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def assert_success(self, result):
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)

    @staticmethod
    def values(arguments, option):
        return [arguments[i + 1] for i, value in enumerate(arguments[:-1]) if value == option]

    def test_prepare_builds_only_the_matching_native_image(self):
        for arch, platform in (("x86_64", "amd64"), ("aarch64", "arm64")):
            with self.subTest(arch=arch):
                target = arch + "-unknown-linux-gnu"
                result = self.invoke(target, "--prepare", STUB_ARCH=arch, LAZYDB_LINUX_BUILD_IMAGE=IMAGES[arch])
                self.assert_success(result)
                self.assertEqual(len(self.calls()), 1)
                call = self.calls()[0]
                self.assertEqual(call[0], "build")
                self.assertEqual(self.values(call, "--platform"), ["linux/" + platform])
                self.assertEqual(self.values(call, "--build-arg"), [
                    "BASE_IMAGE=" + IMAGES[arch], "RUST_TARGET=" + target, "RUSTUP_ARCH=" + arch,
                ])
                self.assertFalse((self.repo / "target" / target / "release/lazydb").exists())

    def test_metadata_has_clean_stdout_and_matches_build_paths_and_environment(self):
        metadata = self.invoke(self.target, "--exec", "cargo", "metadata", "--all-features", "--format-version", "1")
        self.assert_success(metadata)
        self.assertEqual([call[0] for call in self.calls()], ["run"])
        metadata_call = self.calls()[0]
        document = json.loads(metadata.stdout)
        self.assertEqual(document["workspace_root"], str(self.repo))
        self.assertEqual(document["target_directory"], str(self.repo / "target"))
        self.assertTrue(document["packages"][0]["manifest_path"].startswith(str(self.cargo_home) + "/"))
        self.assertFalse((self.repo / "target" / self.target / "release/lazydb").exists())

        result = self.invoke(self.target)
        self.assert_success(result)
        self.assertEqual([call[0] for call in self.calls()], ["build", "run"])
        build_call = self.calls()[-1]
        for option in ("--volume", "--workdir", "--env", "--platform", "--user", "--entrypoint"):
            self.assertEqual(self.values(metadata_call, option), self.values(build_call, option), option)
        self.assertEqual(self.values(build_call, "--volume"), [
            f"{self.repo}:{self.repo}", f"{self.cargo_home}:{self.cargo_home}",
        ])
        self.assertEqual(self.values(build_call, "--workdir"), [str(self.repo)])
        self.assertEqual(self.values(build_call, "--user"), [f"{os.getuid()}:{os.getgid()}"])
        self.assertEqual(self.values(build_call, "--env"), [
            "CARGO_HOME=" + str(self.cargo_home), "CARGO_BUILD_JOBS=2", "CARGO_INCREMENTAL=0",
            "RUSTUP_TOOLCHAIN=1.94.0", "RUSTFLAGS=",
        ])

    def test_exec_preserves_arguments_without_shell_evaluation(self):
        marker = self.directory / "must not exist"
        arguments = ["", "two words", "'quoted'", '"double quoted"', "line\nbreak", "中文",
                     f"$(touch '{marker}')", f"`touch '{marker}'`", f"; touch '{marker}'"]
        result = self.invoke(self.target, "--exec", "python3", "-c",
                             "import json, sys; print(json.dumps(sys.argv[1:]))", *arguments)
        self.assert_success(result)
        self.assertEqual(json.loads(result.stdout), arguments)
        self.assertFalse(marker.exists())
        self.assertEqual([call[0] for call in self.calls()], ["run"])

    def test_exec_returns_command_failure_without_requiring_a_binary(self):
        result = self.invoke(self.target, "--exec", "sh", "-c", "exit 37")
        self.assertEqual(result.returncode, 37, result.stderr)
        self.assertEqual([call[0] for call in self.calls()], ["run"])

    def test_image_build_failure_prevents_container_execution(self):
        for arguments in ((self.target,), (self.target, "--prepare")):
            with self.subTest(arguments=arguments):
                result = self.invoke(*arguments, STUB_BUILD_STATUS="29")
                self.assertEqual(result.returncode, 29, result.stderr)
                self.assertEqual([call[0] for call in self.calls()], ["build"])

    def test_container_failure_is_not_hidden_by_a_stale_binary(self):
        binary = self.repo / "target" / self.target / "release/lazydb"
        binary.parent.mkdir(parents=True)
        binary.write_text("stale binary")
        result = self.invoke(self.target, STUB_RUN_STATUS="41")
        self.assertEqual(result.returncode, 41, result.stderr)

    def test_successful_build_requires_its_output_binary(self):
        result = self.invoke(self.target, STUB_SKIP_BINARY="1")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("build output not found:", result.stderr)

    def test_rejects_invalid_command_modes_before_docker(self):
        for arguments in ((), ("other-target",), (self.target, "--exec"),
                          (self.target, "--prepare", "extra"), (self.target, "--unknown")):
            with self.subTest(arguments=arguments):
                self.assertEqual(self.invoke(*arguments).returncode, 2)
                self.assertEqual(self.calls(), [])

    def test_rejects_cross_architecture_and_unapproved_images_before_docker(self):
        cases = (
            ("aarch64-unknown-linux-gnu", {}, "native runner required"),
            (self.target, {"LAZYDB_LINUX_BUILD_IMAGE": "ubuntu:22.04"}, "pinned by sha256"),
            (self.target, {"LAZYDB_LINUX_BUILD_IMAGE": "ubuntu:22.04@sha256:" + "0" * 64}, "digest does not match"),
            (self.target, {"LAZYDB_LINUX_BUILD_IMAGE": IMAGES["aarch64"]}, "digest does not match"),
        )
        for target, environment, message in cases:
            for mode in (("--prepare",), ("--exec", "cargo", "metadata"), ()):
                with self.subTest(target=target, mode=mode, environment=environment):
                    result = self.invoke(target, *mode, **environment)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(message, result.stderr)
                    self.assertEqual(self.calls(), [])

    def test_rejects_invalid_build_resource_settings_before_docker(self):
        for key, values in (("CARGO_BUILD_JOBS", ("0", "-1", "two")),
                            ("CARGO_INCREMENTAL", ("2", "yes"))):
            for value in values:
                with self.subTest(key=key, value=value):
                    result = self.invoke(self.target, **{key: value})
                    self.assertEqual(result.returncode, 2, result.stderr)
                    self.assertIn(key, result.stderr)
                    self.assertEqual(self.calls(), [])


if __name__ == "__main__":
    unittest.main()
