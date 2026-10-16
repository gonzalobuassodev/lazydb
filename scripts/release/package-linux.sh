#!/bin/sh
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
. "$SCRIPT_DIR/release-lib.sh"
[ "$#" -eq 5 ] || release_die "usage: package-linux.sh VERSION ARCH BINARY RUNTIME_REPORT OUTPUT_DIR"
version=$1; arch=$2; binary=$3; report=$4; output=$5
case "$version" in *-beta.*) release_die 'native packages are stable-only' ;; esac
case "$arch" in x86_64) nfpm_arch=amd64 ;; aarch64) nfpm_arch=arm64 ;; *) release_die "unsupported architecture: $arch" ;; esac
[ -x "$binary" ] || release_die "binary is not executable: $binary"
target="${arch}-unknown-linux-gnu"
[ -f "$report" ] || release_die "missing Linux runtime report: $report"
tmp_report=$(mktemp "${TMPDIR:-/tmp}/lazydb-package-report.XXXXXX")
tmp_package_dir=$(mktemp -d "${TMPDIR:-/tmp}/lazydb-package-stage.XXXXXX")
nfpm_config=
cleanup() { rm -f "$tmp_report"; rm -rf "$tmp_package_dir"; [ -z "$nfpm_config" ] || rm -f "$nfpm_config"; }
trap cleanup EXIT HUP INT TERM
python3 - "$report" "$tmp_report" <<'PY'
import json, sys
from pathlib import Path
data = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
data = {"schema": 1, "product": data["product"], "version": data["version"], "target": data["target"], "archive": {"name": f"lazydb_{data['version']}_{data['target']}.tar.xz", "sha256": "0" * 64}, "binary": data["binary"], "elf": data["elf"], "glibc": data["glibc"], "other_version_needs": data["other_version_needs"]}
Path(sys.argv[2]).write_text(json.dumps(data), encoding="utf-8")
PY
python3 "$SCRIPT_DIR/linux-runtime.py" verify-binary-report --report "$tmp_report" --binary "$binary" --target "$target" --version "$version"
glibc_min=$(python3 - "$report" <<'PY'
import json, sys
print(json.load(open(sys.argv[1], encoding="utf-8"))["glibc"]["loader_min"])
PY
)
export NFPM_DEPENDS_DEB="libc6 (>= $glibc_min), libgcc-s1"
export NFPM_DEPENDS_RPM="glibc >= $glibc_min, libgcc"
export NFPM_DEPENDS_ARCH="glibc>=${glibc_min}"
nfpm_config=$(mktemp "${TMPDIR:-/tmp}/lazydb-nfpm.XXXXXX.yaml")
python3 - "$SCRIPT_DIR/../../packaging/nfpm.yaml" "$nfpm_config" "$glibc_min" <<'PY'
import sys
from pathlib import Path
source, output, version = sys.argv[1:]
text = Path(source).read_text(encoding="utf-8").replace("__GLIBC_MIN__", version)
Path(output).write_text(text, encoding="utf-8")
PY
mkdir -p "$output"
NFPM_VERSION=$version NFPM_ARCH=$nfpm_arch NFPM_BINARY=$binary nfpm pkg --config "$nfpm_config" --packager deb --target "$tmp_package_dir/lazydb_${version}_${nfpm_arch}.deb"
NFPM_VERSION=$version NFPM_ARCH=$nfpm_arch NFPM_BINARY=$binary nfpm pkg --config "$nfpm_config" --packager rpm --target "$tmp_package_dir/lazydb-${version}-${nfpm_arch}.rpm"
NFPM_VERSION=$version NFPM_ARCH=$arch NFPM_BINARY=$binary nfpm pkg --config "$nfpm_config" --packager archlinux --target "$tmp_package_dir/lazydb-${version}-${arch}.pkg.tar.zst"
mkdir -p "$output"
cp "$tmp_package_dir"/* "$output/"
