#!/bin/sh
set -eu
LC_ALL=C
export LC_ALL
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT HUP INT TERM

make_assets() {
    dir=$1
    version=$2
    mkdir -p "$dir"
    for target in x86_64-apple-darwin aarch64-apple-darwin; do
        printf '%s\n' "$target" > "$dir/lazydb_${version}_${target}.tar.xz"
    done
    python3 - "$ROOT" "$dir" "$version" <<'PY'
import hashlib
import importlib.util
import sys
import tarfile
from pathlib import Path

root, directory, release_version = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
spec = importlib.util.spec_from_file_location("linux_runtime", root / "scripts/release/linux-runtime.py")
runtime = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = runtime
spec.loader.exec_module(runtime)
for target in ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"):
    package_root = directory / f"lazydb_{release_version}_{target}"
    package_root.mkdir()
    binary = package_root / "lazydb"
    binary.write_bytes(target.encode())
    archive = directory / f"{package_root.name}.tar.xz"
    with tarfile.open(archive, "w:xz") as package:
        package.add(package_root, arcname=package_root.name)
    libc_version = "2.34" if target.startswith("x86_64") else "2.17"
    data = {
        "elf": {
            "machine": runtime.TARGETS[target]["machine"], "class": "ELF64",
            "endianness": "little", "interpreter": runtime.TARGETS[target]["interpreter"],
            "needed": ["libc.so.6"], "rpath": [], "runpath": [],
        },
        "glibc": {
            "loader_min": libc_version,
            "version_needs": [{"library": "libc.so.6", "version": f"GLIBC_{libc_version}", "weak": False}],
            "symbol_references": [],
        },
        "other_version_needs": [],
    }
    report = runtime.build_report(release_version, target, archive, binary, data)
    (directory / f"{package_root.name}.runtime.json").write_text(runtime.serialize_report(report))
PY
    printf '%s\n' windows > "$dir/lazydb_${version}_x86_64-pc-windows-msvc.zip"
    if [ "$version" = 1.2.3 ]; then
        for name in lazydb_1.2.3_amd64.deb lazydb_1.2.3_arm64.deb lazydb-1.2.3-amd64.rpm lazydb-1.2.3-arm64.rpm lazydb-1.2.3-x86_64.pkg.tar.zst lazydb-1.2.3-aarch64.pkg.tar.zst; do
            printf '%s\n' package > "$dir/$name"
        done
        printf '%s\n' installer > "$dir/lazydb-installer.sh"
        printf '%s\n' installer > "$dir/lazydb-installer.ps1"
    fi
    printf '%s\n' '{}' > "$dir/lazydb-${version}-sbom.spdx.json"
    (cd "$dir" && find . -maxdepth 1 -type f ! -name SHA256SUMS -exec sha256sum {} \; | sed 's#  ./#  #' | sort -k2 > SHA256SUMS)
}

mkdir -p "$TMP/existing" "$TMP/existing/v2" "$TMP/source" "$TMP/stable-assets" "$TMP/beta-assets"
cp "$ROOT/pages/install.sh" "$ROOT/pages/install-beta.sh" "$ROOT/pages/install-core.sh" "$ROOT/pages/install.ps1" "$ROOT/pages/CNAME" "$TMP/source/"
make_assets "$TMP/beta-assets" 1.2.3-beta.1
python3 "$ROOT/scripts/release/generate-channel-manifest.py" beta 1.2.3-beta.1 2026-08-31T12:00:00Z "$TMP/beta-assets" "$TMP/existing/beta.json"
python3 "$ROOT/scripts/release/generate-channel-manifest.py" --schema 2 beta 1.2.3-beta.1 2026-08-31T12:00:00Z "$TMP/beta-assets" "$TMP/existing/v2/beta.json"
make_assets "$TMP/stable-assets" 1.2.3
sh "$ROOT/scripts/release/assemble-pages.sh" stable 1.2.3 2026-08-31T12:00:00Z "$TMP/stable-assets" "$TMP/source" "$TMP/existing" "$TMP/output"
sh "$ROOT/scripts/release/assemble-pages.sh" stable 1.2.3 2026-08-31T12:00:00Z "$TMP/stable-assets" "$TMP/source" "$TMP/existing" "$TMP/output-again"
cmp "$TMP/output/channels/stable.json" "$TMP/output-again/channels/stable.json"
[ -f "$TMP/output/channels/stable.json" ]
[ "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["channel"])' "$TMP/output/channels/beta.json")" = beta ]
[ "$(cat "$TMP/output/CNAME")" = lazydb.yelog.org ]
[ -x "$TMP/output/install.sh" ] && [ -x "$TMP/output/install-beta.sh" ] && [ -x "$TMP/output/install-core.sh" ] && [ -f "$TMP/output/install.ps1" ]
cmp "$TMP/source/install.ps1" "$TMP/output/install.ps1.txt"
cmp "$TMP/output/install.ps1" "$TMP/output/install.ps1.txt"
[ "$(grep -c 'set -eu' "$TMP/output/install.sh")" -eq 1 ]
[ "$(grep -c 'LAZYDB_CHANNEL_LOCKED=stable' "$TMP/output/install.sh")" -eq 1 ]
[ "$(grep -c 'LAZYDB_CHANNEL_LOCKED=beta' "$TMP/output/install-beta.sh")" -eq 1 ]
[ "$(grep -c 'Configure LazyDB MCP now?' "$TMP/output/install.sh")" -eq 1 ]
[ "$(grep -c 'Configure LazyDB MCP now?' "$TMP/output/install-beta.sh")" -eq 1 ]
[ "$(grep -c 'mcp setup' "$TMP/output/install.sh")" -ge 1 ]
[ "$(find "$TMP/output" -type f | sort | sed "s#^$TMP/output/##")" = "CNAME
channels/beta.json
channels/stable.json
channels/v2/beta.json
channels/v2/stable.json
install-beta.sh
install-core.sh
install.ps1
install.ps1.txt
install.sh" ]
if find "$TMP/output" -type f -name '*.tar.xz' | grep . >/dev/null 2>&1; then exit 1; fi

cp "$TMP/output/channels/stable.json" "$TMP/existing/stable.json"
mkdir -p "$TMP/existing/v2"
cp "$TMP/output/channels/v2/stable.json" "$TMP/existing/v2/stable.json"
sh "$ROOT/scripts/release/assemble-pages.sh" beta 1.2.3-beta.1 2026-08-31T12:00:00Z "$TMP/beta-assets" "$TMP/source" "$TMP/existing" "$TMP/output-beta"
[ -f "$TMP/output-beta/channels/beta.json" ]
[ -f "$TMP/output-beta/channels/v2/beta.json" ]
cmp "$TMP/source/install.ps1" "$TMP/output-beta/install.ps1.txt"
[ "$(cat "$TMP/output-beta/channels/stable.json")" = "$(cat "$TMP/existing/stable.json")" ]
[ "$(cat "$TMP/output-beta/channels/v2/stable.json")" = "$(cat "$TMP/existing/v2/stable.json")" ]
[ "$(cat "$TMP/output-beta/CNAME")" = lazydb.yelog.org ]
if find "$TMP/output-beta" -type f \( -name '*.tar.xz' -o -name '*.deb' -o -name '*.rpm' \) | grep . >/dev/null 2>&1; then exit 1; fi

# A first deployment may have only one channel; the next release completes it.
rm -f "$TMP/existing/beta.json"
rm -f "$TMP/existing/v2/beta.json"
sh "$ROOT/scripts/release/assemble-pages.sh" stable 1.2.3 2026-08-31T12:00:00Z "$TMP/stable-assets" "$TMP/source" "$TMP/existing" "$TMP/output-first"
[ -f "$TMP/output-first/channels/stable.json" ]
[ -f "$TMP/output-first/channels/v2/stable.json" ]
[ ! -e "$TMP/output-first/channels/beta.json" ]
[ ! -e "$TMP/output-first/channels/v2/beta.json" ]
printf '%s\n' 'Pages tests: ok'
