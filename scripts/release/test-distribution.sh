#!/bin/sh
set -eu
SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
python3 -B "$SCRIPT_DIR/test-next-version.py"
python3 -B "$SCRIPT_DIR/test-linux-runtime.py"
python3 -B "$SCRIPT_DIR/test-build-linux.py"
python3 -B "$SCRIPT_DIR/../ci/test-rust-cache-command.py"
for test in test-archive test-macos-dependencies test-installer test-channel-manifest test-pages test-release-metadata test-online-install test-linux-packages test-linux-runtime-smoke; do
    printf 'Running %s\n' "$test"
    sh "$SCRIPT_DIR/$test.sh"
done
