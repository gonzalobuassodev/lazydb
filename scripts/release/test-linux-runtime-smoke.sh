#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT HUP INT TERM
if sh "$ROOT/scripts/release/smoke-linux-runtime.sh" >"$TMP/output" 2>&1; then
    echo 'runtime smoke accepted missing arguments' >&2; exit 1
fi
grep -q 'usage: smoke-linux-runtime.sh' "$TMP/output"
printf '%s\n' '{' > "$TMP/malformed.json"
if sh "$ROOT/scripts/release/smoke-linux-runtime.sh" /dev/null "$TMP/malformed.json" 1.2.3 x86_64-unknown-linux-gnu "$TMP/evidence.json" >"$TMP/output" 2>&1; then
    echo 'runtime smoke accepted an invalid archive/report' >&2; exit 1
fi
grep -q 'could not read runtime report' "$TMP/output"
[ ! -e "$TMP/evidence.json" ]
python3 - "$ROOT/scripts/release/smoke-linux-runtime.sh" <<'PY'
import sys
from pathlib import Path
script=Path(sys.argv[1]).read_text(encoding="utf-8")
assert '"update_check":json.loads(Path(tmp,"update.json.summary")' in script
assert '"exit_code":int(sys.argv[3])' in script
assert '"status":None if report is None else report["status"]' in script
PY
echo 'Linux runtime smoke interface tests: ok'
