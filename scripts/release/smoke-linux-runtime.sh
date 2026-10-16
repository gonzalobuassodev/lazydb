#!/bin/sh
set -eu
[ "$#" -eq 5 ] || { echo 'usage: smoke-linux-runtime.sh ARCHIVE REPORT VERSION TARGET EVIDENCE_OUTPUT' >&2; exit 2; }
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
archive=$1 report=$2 version=$3 target=$4 evidence=$5
python3 "$SCRIPT_DIR/linux-runtime.py" verify-report --report "$report" --archive "$archive" --target "$target" --version "$version"
case "$target" in
  x86_64-unknown-linux-gnu) expected_arch=x86_64 ;;
  aarch64-unknown-linux-gnu) expected_arch=aarch64 ;;
  *) echo "unsupported Linux smoke target: $target" >&2; exit 1 ;;
esac
[ "$(uname -m)" = "$expected_arch" ] || { echo "native smoke requires $expected_arch runner" >&2; exit 1; }
tmp=$(mktemp -d "${TMPDIR:-/tmp}/lazydb-runtime-smoke.XXXXXX")
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
root="lazydb_${version}_${target}"
tar -xJf "$archive" -C "$tmp" "$root/lazydb"
binary="$tmp/$root/lazydb"
chmod 0755 "$binary"
mkdir -p "$tmp/home" "$tmp/config" "$tmp/data" "$tmp/project"
HOME="$tmp/home" XDG_CONFIG_HOME="$tmp/config" XDG_DATA_HOME="$tmp/data" "$binary" version --json > "$tmp/version.json"
python3 - "$tmp/version.json" "$version" <<'PY'
import json,sys
if json.load(open(sys.argv[1], encoding="utf-8")).get("version") != sys.argv[2]:
    raise SystemExit("archive version command returned the wrong version")
PY
doctor_status=0
HOME="$tmp/home" XDG_CONFIG_HOME="$tmp/config" XDG_DATA_HOME="$tmp/data" "$binary" doctor --json > "$tmp/doctor.json" || doctor_status=$?
printf '%s\n' "$doctor_status" > "$tmp/doctor-status"
python3 - "$tmp/doctor.json" <<'PY'
import json,sys
report=json.load(open(sys.argv[1], encoding="utf-8"))
if not isinstance(report,dict) or 'ok' not in report: raise SystemExit("doctor returned an unexpected report")
PY
HOME="$tmp/home" XDG_CONFIG_HOME="$tmp/config" XDG_DATA_HOME="$tmp/data" "$binary" agent execute --sql 'SELECT 1 AS value' --url "sqlite://$tmp/smoke.db" --json > "$tmp/query.json"
python3 - "$tmp/query.json" <<'PY'
import json,sys
value=json.load(open(sys.argv[1], encoding="utf-8"))
def find_one(node):
    if isinstance(node, dict):
        if node.get("value") == 1: return True
        return any(find_one(child) for child in node.values())
    if isinstance(node, list): return any(find_one(child) for child in node)
    return False
if not find_one(value): raise SystemExit("SQLite smoke query did not return value=1")
PY
update_status=0
HOME="$tmp/home" XDG_CONFIG_HOME="$tmp/config" XDG_DATA_HOME="$tmp/data" \
  LAZYDB_CHANNEL_BASE_URL=https://lazydb.yelog.org/channels \
  "$binary" update --check --json > "$tmp/update.json" 2> "$tmp/update-error" || update_status=$?
python3 - "$tmp/update.json" "$tmp/update-error" "$update_status" <<'PY'
import json,sys
try:
    report=json.load(open(sys.argv[1], encoding="utf-8"))
except (OSError, json.JSONDecodeError):
    report=None
if report is None and int(sys.argv[3]) == 0:
    raise SystemExit("update check exited successfully without a JSON report")
if report is not None and report.get("status") not in {"up_to_date", "update_available", "manager_action_required", "error"}:
    raise SystemExit("update check returned an unknown status")
with open(sys.argv[1]+".summary", "w", encoding="utf-8") as stream:
    json.dump({"exit_code":int(sys.argv[3]),"status":None if report is None else report["status"]}, stream, sort_keys=True)
PY
python3 - "$report" "$version" "$target" "$evidence" "$tmp" <<'PY'
import datetime,json,platform,sys
from pathlib import Path
report,version,target,evidence,tmp=sys.argv[1:]
data=json.load(open(report,encoding="utf-8"))
result={"schema":1,"product":"lazydb","version":version,"target":target,"archive_sha256":data["archive"]["sha256"],"binary_sha256":data["binary"]["sha256"],"runner_arch":platform.machine(),"glibc":platform.libc_ver()[1],"native":True,"checks":{"version":True,"doctor_json":True,"doctor_exit_code":int(Path(tmp,"doctor-status").read_text()),"sqlite_select_1":True,"update_check":json.loads(Path(tmp,"update.json.summary").read_text(encoding="utf-8"))},"recorded_at":datetime.datetime.now(datetime.timezone.utc).isoformat()}
Path(evidence).parent.mkdir(parents=True,exist_ok=True)
Path(evidence).write_text(json.dumps(result,indent=2)+"\n",encoding="utf-8")
PY
