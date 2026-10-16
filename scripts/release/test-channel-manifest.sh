#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
GENERATOR="$ROOT/scripts/release/generate-channel-manifest.py"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT HUP INT TERM

make_fixture() {
    dir=$1
    version=$2
    mkdir -p "$dir"
    for target in x86_64-apple-darwin aarch64-apple-darwin x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu; do
        printf '%s\n' "$target" > "$dir/lazydb_${version}_${target}.tar.xz"
    done
    printf '%s\n' windows > "$dir/lazydb_${version}_x86_64-pc-windows-msvc.zip"
    python3 - "$ROOT" "$dir" "$version" <<'PY'
import hashlib
import importlib.util
import json
import sys
import tarfile
from pathlib import Path

root, directory, version = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
sys.path.insert(0, str(root / "scripts/release"))
spec = importlib.util.spec_from_file_location("linux_runtime", root / "scripts/release/linux-runtime.py")
runtime = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = runtime
spec.loader.exec_module(runtime)
for target in ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"):
    archive = directory / f"lazydb_{version}_{target}.tar.xz"
    package_root = directory / f"lazydb_{version}_{target}"
    package_root.mkdir(exist_ok=True)
    binary = package_root / "lazydb"
    binary.write_bytes(target.encode())
    with tarfile.open(archive, "w:xz") as package:
        package.add(package_root, arcname=package_root.name)
    parsed = {
        "elf": {
            "machine": runtime.TARGETS[target]["machine"],
            "class": "ELF64",
            "endianness": "little",
            "interpreter": runtime.TARGETS[target]["interpreter"],
            "needed": ["libc.so.6"],
            "rpath": [],
            "runpath": [],
        },
        "glibc": {
            "loader_min": "2.34" if target.startswith("x86_64") else "2.17",
            "version_needs": [{"library": "libc.so.6", "version": "GLIBC_2.34" if target.startswith("x86_64") else "GLIBC_2.17", "weak": False}],
            "symbol_references": [],
        },
        "other_version_needs": [],
    }
    report = runtime.build_report(version, target, archive, binary, parsed)
    report_path = directory / f"lazydb_{version}_{target}.runtime.json"
    report_path.write_text(runtime.serialize_report(report))
files = sorted(path for path in directory.iterdir() if path.is_file() and path.name != "SHA256SUMS")
(directory / "SHA256SUMS").write_text("".join(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n" for path in files))
PY
}

assert_rejects() {
    if "$@" >/dev/null 2>&1; then
        printf 'expected rejection: %s\n' "$*" >&2
        exit 1
    fi
}

stable="$TMP/stable"
beta="$TMP/beta"
make_fixture "$stable" 1.2.3
make_fixture "$beta" 1.2.3-beta.2
python3 "$GENERATOR" stable 1.2.3 2026-08-31T12:00:00Z "$stable" "$TMP/stable.json"
python3 "$GENERATOR" beta 1.2.3-beta.2 2026-08-31T12:00:00Z "$beta" "$TMP/beta.json"
python3 - "$TMP/stable.json" "$TMP/beta.json" <<'PY'
import json
import sys

for path, channel, version in ((sys.argv[1], "stable", "1.2.3"), (sys.argv[2], "beta", "1.2.3-beta.2")):
    data = json.load(open(path, encoding="utf-8"))
    assert set(data) == {"schema", "product", "channel", "version", "tag", "prerelease", "published_at", "release_url", "assets"}
    assert data["schema"] == 1 and data["product"] == "lazydb"
    assert data["channel"] == channel and data["version"] == version
    assert data["tag"] == "v" + version and data["prerelease"] == (channel == "beta")
    assert data["release_url"].startswith("https://github.com/yelog/lazydb/")
    assert set(data["assets"]) == {"x86_64-apple-darwin", "aarch64-apple-darwin", "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "x86_64-pc-windows-msvc"}
    for asset in data["assets"].values():
        assert asset["url"].startswith("https://github.com/yelog/lazydb/releases/download/")
        assert "evil.example" not in asset["url"]
        assert len(asset["sha256"]) == 64
PY

mv "$stable/lazydb_1.2.3_x86_64-unknown-linux-gnu.runtime.json" "$TMP/missing-runtime.json"
assert_rejects python3 "$GENERATOR" --schema 2 stable 1.2.3 2026-08-31T12:00:00Z "$stable" "$TMP/no-v2.json"
mv "$TMP/missing-runtime.json" "$stable/lazydb_1.2.3_x86_64-unknown-linux-gnu.runtime.json"
python3 "$GENERATOR" --schema 2 stable 1.2.3 2026-08-31T12:00:00Z "$stable" "$TMP/stable-v2.json"
python3 - "$TMP/stable-v2.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
assert data["schema"] == 2
assert data["assets"]["x86_64-unknown-linux-gnu"]["requirements"]["libc"] == {"family": "glibc", "min_version": "2.34"}
assert data["assets"]["aarch64-unknown-linux-gnu"]["requirements"]["libc"] == {"family": "glibc", "min_version": "2.17"}
assert "requirements" not in data["assets"]["x86_64-apple-darwin"]
assert "requirements" not in data["assets"]["x86_64-pc-windows-msvc"]
PY
cp "$stable/lazydb_1.2.3_x86_64-unknown-linux-gnu.runtime.json" "$TMP/report.original"
printf '%s\n' '{}' > "$stable/lazydb_1.2.3_x86_64-unknown-linux-gnu.runtime.json"
assert_rejects python3 "$GENERATOR" --schema 2 stable 1.2.3 2026-08-31T12:00:00Z "$stable" "$TMP/tampered-v2.json"
cp "$TMP/report.original" "$stable/lazydb_1.2.3_x86_64-unknown-linux-gnu.runtime.json"

assert_rejects python3 "$GENERATOR" stable 1.2.3-beta.2 now "$beta" "$TMP/no.json"
assert_rejects python3 "$GENERATOR" beta 1.2.3 now "$stable" "$TMP/no.json"
rm "$stable/lazydb_1.2.3_x86_64-apple-darwin.tar.xz"
assert_rejects python3 "$GENERATOR" stable 1.2.3 now "$stable" "$TMP/no.json"
make_fixture "$stable" 1.2.3
grep -v 'aarch64-unknown-linux-gnu' "$stable/SHA256SUMS" > "$TMP/missing"
mv "$TMP/missing" "$stable/SHA256SUMS"
assert_rejects python3 "$GENERATOR" stable 1.2.3 now "$stable" "$TMP/no.json"
make_fixture "$stable" 1.2.3
printf '%s\n' "$(sha256sum "$stable/lazydb_1.2.3_x86_64-apple-darwin.tar.xz" | awk '{print $1}') lazydb_1.2.3_x86_64-apple-darwin.tar.xz" >> "$stable/SHA256SUMS"
assert_rejects python3 "$GENERATOR" stable 1.2.3 now "$stable" "$TMP/no.json"
assert_rejects python3 "$GENERATOR" stable 1.2.3-beta.0 now "$stable" "$TMP/no.json"
printf '%s\n' 'channel manifest tests: ok'
