#!/bin/sh
set -eu

# rust-cache hashes every toolchain returned by this command. Hosted runners
# can have different unused stable versions even within one workflow run.
# All builds use RUSTUP_TOOLCHAIN, so only that compiler belongs in this cache.
if [ "$#" -eq 4 ] && [ "$1" = rustup ] && [ "$2" = toolchain ] &&
    [ "$3" = list ] && [ "$4" = --quiet ]; then
    printf '%s\n' "${RUSTUP_TOOLCHAIN:?RUSTUP_TOOLCHAIN must identify the CI compiler}"
else
    exec "$@"
fi
