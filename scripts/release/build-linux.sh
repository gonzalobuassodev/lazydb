#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPOSITORY=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)

[ "$#" -eq 1 ] || { printf 'Usage: build-linux.sh TARGET\n' >&2; exit 2; }
target=$1
case "$target" in
    x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu) ;;
    *) printf 'Unsupported Linux target: %s\n' "$target" >&2; exit 2 ;;
esac

host_arch=$(uname -m)
case "$target:$host_arch" in
    x86_64-unknown-linux-gnu:x86_64|aarch64-unknown-linux-gnu:aarch64) ;;
    *) printf 'native runner required: target=%s host=%s\n' "$target" "$host_arch" >&2; exit 1 ;;
esac

image=${LAZYDB_LINUX_BUILD_IMAGE:-}
case "$image" in
    *@sha256:[0-9a-f][0-9a-f]*) ;;
    *) printf 'LAZYDB_LINUX_BUILD_IMAGE must be pinned by sha256 digest\n' >&2; exit 1 ;;
esac

command -v docker >/dev/null 2>&1 || { printf 'docker is required\n' >&2; exit 1; }

jobs=${CARGO_BUILD_JOBS:-1}
case "$jobs" in ''|*[!0-9]*|0) printf 'CARGO_BUILD_JOBS must be a positive integer\n' >&2; exit 2;; esac
incremental=${CARGO_INCREMENTAL:-0}
case "$incremental" in 0|1) ;; *) printf 'CARGO_INCREMENTAL must be 0 or 1\n' >&2; exit 2;; esac

image_digest=${image##*@}
case "$host_arch:$image_digest" in
    x86_64:sha256:f610ab94648195aa356059f5b41d6085c9d4d903c072430cdd1af7bdb646106b) ;;
    aarch64:sha256:08571ca13e00ca07a2a84eab83a959b4242e22cceb16486a11bef1428c9e93a7) ;;
    *) printf 'build image digest does not match native architecture %s\n' "$host_arch" >&2; exit 1;;
esac

platform=amd64
[ "$host_arch" != aarch64 ] || platform=arm64

rustup_arch=x86_64
[ "$host_arch" != aarch64 ] || rustup_arch=aarch64

repo=$(cd "$REPOSITORY" && pwd -P)
cargo_home=${CARGO_HOME:-"$HOME/.cargo"}
mkdir -p "$repo/target" "$repo/.cargo" "$cargo_home/registry" "$cargo_home/git"
image_tag="lazydb-linux-build:${host_arch}"
docker build \
    --platform "linux/$platform" \
    --build-arg "BASE_IMAGE=$image" \
    --build-arg "RUST_TARGET=$target" \
    --build-arg "RUSTUP_ARCH=$rustup_arch" \
    --file "$SCRIPT_DIR/Dockerfile.linux" \
    --tag "$image_tag" \
    "$SCRIPT_DIR"
set +e
docker run --rm \
    --volume "$repo:/workspace" \
    --volume "$cargo_home/registry:/opt/cargo/registry" \
    --volume "$cargo_home/git:/opt/cargo/git" \
    --workdir /workspace \
    --platform "linux/$platform" \
    --env CARGO_BUILD_JOBS="$jobs" \
    --env "CARGO_INCREMENTAL=$incremental" \
    --env RUSTFLAGS= \
    "$image_tag" \
    "rustc +1.94.0 --version && cargo +1.94.0 --version && readelf --version | head -n 1 && cargo +1.94.0 build --release --locked --target '$target'"
build_status=$?
set -e

[ "$build_status" -eq 0 ] || exit "$build_status"

binary="$repo/target/$target/release/lazydb"
[ -f "$binary" ] || { printf 'build output not found: %s\n' "$binary" >&2; exit 1; }
