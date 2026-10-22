#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT HUP INT TERM

mkdir -p "$TMP/bin" "$TMP/server/channels" "$TMP/server/assets" "$TMP/home" "$TMP/install" "$TMP/beta-home" "$TMP/beta-install" "$TMP/pages"
cp "$ROOT/pages/install.sh" "$ROOT/pages/install-beta.sh" "$ROOT/pages/install-core.sh" "$TMP/pages/"
cat > "$TMP/bin/curl" <<'SH'
#!/bin/sh
set -eu
out=
url=
status_output=
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) shift; out=$1 ;;
        -w) shift; status_output=$1 ;;
        http*) url=$1 ;;
    esac
    shift
done
case "$url" in
  */v2/*)
    case "${FIXTURE_V2_STATUS:-404}" in
      200) src="$TMPDIR_TEST/server/channels/v2/${url##*/}"; if [ -f "$src" ]; then cp "$src" "$out"; else cp "$TMPDIR_TEST/server/channels/${url##*/}" "$out"; fi; printf 200; exit 0 ;;
      404) printf 404; exit 0 ;;
      *) printf '%s' "${FIXTURE_V2_STATUS}"; exit 0 ;;
    esac ;;
esac
case "$url" in
  *channels/*) src="$TMPDIR_TEST/server/${url##*/}"; src="$TMPDIR_TEST/server/channels/${url##*/}" ;;
  *releases/download/*) src="$TMPDIR_TEST/server/assets/${url##*/}" ;;
  *) exit 1 ;;
esac
cp "$src" "$out"
[ -z "$status_output" ] || printf 200
SH
chmod 755 "$TMP/bin/curl"
cat > "$TMP/bin/uname" <<'SH'
#!/bin/sh
case "${1:-}" in -s) printf '%s\n' Linux ;; -m) printf '%s\n' x86_64 ;; esac
SH
chmod 755 "$TMP/bin/uname"
# Use deterministic fake binaries so the test does not require a release build.
mkdir -p "$TMP/assets/stable" "$TMP/assets/beta"
cat > "$TMP/assets/lazydb" <<'SH'
#!/bin/sh
[ "${1:-}" = version ] && printf '%s\n' '{"version":"1.2.3"}'
SH
chmod 755 "$TMP/assets/lazydb"
mkdir -p "$TMP/package/lazydb_1.2.3_x86_64-unknown-linux-gnu"
cp "$TMP/assets/lazydb" "$TMP/package/lazydb_1.2.3_x86_64-unknown-linux-gnu/lazydb"
(cd "$TMP/package" && COPYFILE_DISABLE=1 tar -cJf "$TMP/server/assets/lazydb_1.2.3_x86_64-unknown-linux-gnu.tar.xz" lazydb_1.2.3_x86_64-unknown-linux-gnu)
for target in x86_64-apple-darwin aarch64-apple-darwin aarch64-unknown-linux-gnu; do
    cp "$TMP/server/assets/lazydb_1.2.3_x86_64-unknown-linux-gnu.tar.xz" "$TMP/server/assets/lazydb_1.2.3_$target.tar.xz"
done
printf '%s\n' windows > "$TMP/server/assets/lazydb_1.2.3_x86_64-pc-windows-msvc.zip"
(cd "$TMP/server/assets" && sha256sum lazydb_*.tar.xz lazydb_*.zip > SHA256SUMS)
python3 "$ROOT/scripts/release/generate-channel-manifest.py" stable 1.2.3 2026-09-05T00:00:00Z "$TMP/server/assets" "$TMP/server/channels/stable.json"
mkdir -p "$TMP/server/channels/v2"
cp "$TMP/server/channels/stable.json" "$TMP/server/channels/v2/stable.json"
python3 - "$TMP/server/channels/v2/stable.json" <<'PY'
import json,sys
from pathlib import Path
path=Path(sys.argv[1]); data=json.loads(path.read_text())
data['schema']=2
for target in ('x86_64-unknown-linux-gnu','aarch64-unknown-linux-gnu'):
    data['assets'][target]['requirements']={'libc':{'family':'glibc','min_version':'2.34'}}
path.write_text(json.dumps(data))
PY
cat > "$TMP/assets/beta-lazydb" <<'SH'
#!/bin/sh
[ "${1:-}" = version ] && printf '%s\n' '{"version":"1.2.3-beta.1"}'
SH
chmod 755 "$TMP/assets/beta-lazydb"
mkdir -p "$TMP/package/lazydb_1.2.3-beta.1_x86_64-unknown-linux-gnu"
cp "$TMP/assets/beta-lazydb" "$TMP/package/lazydb_1.2.3-beta.1_x86_64-unknown-linux-gnu/lazydb"
(cd "$TMP/package" && COPYFILE_DISABLE=1 tar -cJf "$TMP/server/assets/lazydb_1.2.3-beta.1_x86_64-unknown-linux-gnu.tar.xz" lazydb_1.2.3-beta.1_x86_64-unknown-linux-gnu)
for target in x86_64-apple-darwin aarch64-apple-darwin aarch64-unknown-linux-gnu; do
    cp "$TMP/server/assets/lazydb_1.2.3-beta.1_x86_64-unknown-linux-gnu.tar.xz" "$TMP/server/assets/lazydb_1.2.3-beta.1_$target.tar.xz"
done
printf '%s\n' windows > "$TMP/server/assets/lazydb_1.2.3-beta.1_x86_64-pc-windows-msvc.zip"
(cd "$TMP/server/assets" && sha256sum lazydb_*.tar.xz lazydb_*.zip > SHA256SUMS)
python3 "$ROOT/scripts/release/generate-channel-manifest.py" beta 1.2.3-beta.1 2026-09-05T00:00:00Z "$TMP/server/assets" "$TMP/server/channels/beta.json"
cp "$TMP/server/channels/beta.json" "$TMP/server/channels/v2/beta.json"
export TMPDIR_TEST="$TMP" PATH="$TMP/bin:$PATH" LAZYDB_CHANNEL_BASE_URL=https://fixture/channels
export LAZYDB_CONFIG_HOME="$TMP/home/config"
export SHELL=/bin/bash LAZYDB_MCP_SETUP=skip
unset ZDOTDIR XDG_CONFIG_HOME LAZYDB_INSTALL_DIR
REAL_PYTHON=$(command -v python3)
mkdir -p "$TMP/no-lzma-bin"
cat > "$TMP/no-lzma-bin/python3" <<SH
#!/bin/sh
if [ "\${1:-}" = -c ] && printf '%s' "\${2:-}" | grep -q 'import lzma'; then
    exit 1
fi
exec "$REAL_PYTHON" "\$@"
SH
chmod 755 "$TMP/no-lzma-bin/python3"
if HOME="$TMP/no-lzma-home" PATH="$TMP/no-lzma-bin:$TMP/bin:$PATH" \
    LAZYDB_CONFIG_HOME="$TMP/no-lzma-home/config" \
    sh "$TMP/pages/install.sh" --install-dir "$TMP/no-lzma-install" >"$TMP/no-lzma-output" 2>&1; then
    printf '%s\n' 'installer accepted Python without XZ support' >&2
    exit 1
fi
grep -q 'cannot decode XZ archives' "$TMP/no-lzma-output"
[ ! -e "$TMP/no-lzma-home/config/install.json" ]
# Consume the same five-target manifests as production, including Windows.
if ! HOME="$TMP/home" sh "$TMP/pages/install.sh" --install-dir "$TMP/install" >/dev/null; then
    printf '%s\n' 'installer fixture failed' >&2
    exit 1
fi
[ -L "$TMP/home/config/current" ]
[ -L "$TMP/install/lazydb" ]
[ -f "$TMP/home/config/install.json" ]
[ "$(python3 -c 'import json; print(json.load(open("'$TMP'/home/config/install.json"))["channel"])')" = stable ]
[ -e "$TMP/install/lazydb" ]
if HOME="$TMP/reject-home" LAZYDB_CONFIG_HOME="$TMP/reject-home/config" LAZYDB_INSTALL_TEST_LOG="$TMP/reject-downloads" FIXTURE_V2_STATUS=500 sh "$TMP/pages/install.sh" --install-dir "$TMP/reject-install" >"$TMP/reject-output" 2>&1; then
    printf '%s\n' 'installer accepted an HTTP 500 manifest response' >&2; exit 1
fi
[ "$(wc -l < "$TMP/reject-downloads" | tr -d ' ')" = 1 ]
[ ! -e "$TMP/reject-home/config/install.json" ]
[ ! -e "$TMP/reject-home/config/current" ]
cat > "$TMP/bin/getconf" <<'SH'
#!/bin/sh
if [ "${FIXTURE_GLIBC_VERSION:-2.17}" = invalid ]; then printf '%s\n' 'glibc unknown'; else printf 'glibc %s\n' "${FIXTURE_GLIBC_VERSION:-2.17}"; fi
SH
chmod 755 "$TMP/bin/getconf"
for installer in "$TMP/pages/install.sh" "$ROOT/install.sh"; do
    for glibc_version in 2.34 2.40; do
        test_home="$TMP/v2-$(basename "$(dirname "$installer")")-$glibc_version"
        if ! HOME="$test_home" LAZYDB_CONFIG_HOME="$test_home/config" \
            LAZYDB_INSTALL_TEST_LOG="$test_home/downloads" FIXTURE_V2_STATUS=200 \
            FIXTURE_GLIBC_VERSION="$glibc_version" \
            sh "$installer" --version 1.2.3 --install-dir "$test_home/bin" --no-modify-path >"$TMP/v2-output" 2>&1; then
            printf 'v2 installer failed with compatible glibc %s: %s\n' "$glibc_version" "$installer" >&2
            cat "$TMP/v2-output" >&2
            exit 1
        fi
        python3 - "$test_home" <<'PY'
import json,subprocess,sys
from pathlib import Path
home=Path(sys.argv[1])
state=json.loads((home/'config/install.json').read_text())
assert state['version'] == '1.2.3' and state['channel'] == 'stable'
assert state['target'] == 'x86_64-unknown-linux-gnu'
assert state['path'] == str(home/'bin/lazydb')
assert (home/'config/current').resolve() == (home/'config/releases/1.2.3').resolve()
assert json.loads(subprocess.check_output([str(home/'bin/lazydb'),'version','--json'],text=True))['version'] == '1.2.3'
assert (home/'downloads').read_text().splitlines() == [
    'https://fixture/channels/v2/stable.json',
    'https://github.com/yelog/lazydb/releases/download/v1.2.3/lazydb_1.2.3_x86_64-unknown-linux-gnu.tar.xz',
]
PY
    done
done
if HOME="$TMP/incompatible-home" LAZYDB_CONFIG_HOME="$TMP/incompatible-home/config" LAZYDB_INSTALL_TEST_LOG="$TMP/incompatible-downloads" FIXTURE_V2_STATUS=200 sh "$TMP/pages/install.sh" --install-dir "$TMP/incompatible-install" >"$TMP/incompatible-output" 2>&1; then
    printf '%s\n' 'installer accepted an incompatible glibc runtime' >&2; exit 1
fi
grep -q 'requires glibc' "$TMP/incompatible-output"
[ "$(wc -l < "$TMP/incompatible-downloads" | tr -d ' ')" = 1 ]
[ ! -e "$TMP/incompatible-home/config/install.json" ]
[ ! -e "$TMP/incompatible-home/config/current" ]
if HOME="$TMP/numeric-home" LAZYDB_CONFIG_HOME="$TMP/numeric-home/config" LAZYDB_INSTALL_TEST_LOG="$TMP/numeric-downloads" FIXTURE_V2_STATUS=200 FIXTURE_GLIBC_VERSION=2.9 sh "$TMP/pages/install.sh" --install-dir "$TMP/numeric-install" >"$TMP/numeric-output" 2>&1; then
    printf '%s\n' 'installer accepted glibc 2.9 for a 2.34 requirement' >&2; exit 1
fi
[ "$(wc -l < "$TMP/numeric-downloads" | tr -d ' ')" = 1 ]
[ ! -e "$TMP/numeric-home/config/install.json" ]
if HOME="$TMP/malformed-libc-home" LAZYDB_CONFIG_HOME="$TMP/malformed-libc-home/config" LAZYDB_INSTALL_TEST_LOG="$TMP/malformed-libc-downloads" FIXTURE_V2_STATUS=200 FIXTURE_GLIBC_VERSION=invalid sh "$TMP/pages/install.sh" --install-dir "$TMP/malformed-libc-install" >"$TMP/malformed-libc-output" 2>&1; then
    printf '%s\n' 'installer accepted an unparseable glibc version' >&2; exit 1
fi
grep -q 'invalid glibc version returned by getconf' "$TMP/malformed-libc-output"
[ "$(wc -l < "$TMP/malformed-libc-downloads" | tr -d ' ')" = 1 ]
[ ! -e "$TMP/malformed-libc-home/config/install.json" ]
HOME="$TMP/home" sh "$TMP/pages/install.sh" --install-dir "$TMP/install" >/dev/null
[ -d "$TMP/home/config/releases/1.2.3" ]
if HOME="$TMP/home" sh "$TMP/pages/install.sh" --channel invalid --install-dir "$TMP/install" >/dev/null 2>&1; then
    printf '%s\n' 'invalid channel was accepted' >&2
    exit 1
fi
if HOME="$TMP/home" sh "$TMP/pages/install.sh" --channel beta --install-dir "$TMP/install" >/dev/null 2>&1; then
    printf '%s\n' 'stable entrypoint allowed beta channel' >&2
    exit 1
fi
beta_output=$(HOME="$TMP/beta-home" LAZYDB_CONFIG_HOME="$TMP/beta-home/config" sh "$TMP/pages/install-beta.sh" --install-dir "$TMP/beta-install")
case "$beta_output" in
    *'LAZYDB BETA installer'*'lazydb 1.2.3-beta.1 installed (beta)'*) ;;
    *) printf 'unexpected beta output: %s\n' "$beta_output" >&2; exit 1 ;;
esac
[ "$(python3 -c 'import json; print(json.load(open("'$TMP'/beta-home/config/install.json"))["channel"])')" = beta ]
HOME="$TMP/home" XDG_DATA_HOME="$TMP/root-data" sh "$ROOT/install.sh" --install-dir "$TMP/root-install" >/dev/null
[ ! -e "$TMP/fresh-home/.config/lazydb" ]
HOME="$TMP/fresh-home" LAZYDB_CONFIG_HOME= XDG_DATA_HOME="$TMP/fresh-data" \
    sh "$ROOT/install.sh" --install-dir "$TMP/fresh-install" >/dev/null
[ -L "$TMP/fresh-home/lazydb/current" ]
[ -f "$TMP/fresh-home/lazydb/install.json" ]
[ ! -e "$TMP/fresh-data/lazydb" ]
mkdir -p "$TMP/preferred-home/lazydb" "$TMP/preferred-home/.config/lazydb"
printf '%s\n' preferred > "$TMP/preferred-home/lazydb/settings.toml"
printf '%s\n' legacy > "$TMP/preferred-home/.config/lazydb/settings.toml"
HOME="$TMP/preferred-home" LAZYDB_CONFIG_HOME= \
    sh "$ROOT/install.sh" --install-dir "$TMP/preferred-install" >/dev/null
[ -f "$TMP/preferred-home/lazydb/install.json" ]
[ ! -e "$TMP/preferred-home/.config/lazydb/install.json" ]
mkdir -p "$TMP/legacy-home/.config/lazydb"
printf '%s\n' legacy > "$TMP/legacy-home/.config/lazydb/settings.toml"
HOME="$TMP/legacy-home" LAZYDB_CONFIG_HOME= \
    sh "$ROOT/install.sh" --install-dir "$TMP/legacy-install" >/dev/null
[ -f "$TMP/legacy-home/.config/lazydb/install.json" ]
[ ! -e "$TMP/legacy-home/lazydb/install.json" ]
for installer in "$TMP/pages/install.sh" "$ROOT/install.sh"; do
    test_home="$TMP/path-$(basename "$(dirname "$installer")")"
    mkdir -p "$test_home"
    original_path=$PATH
    HOME="$test_home" sh "$installer" > "$TMP/path-output"
    [ "$PATH" = "$original_path" ]
    grep -q 'Run in your current terminal' "$TMP/path-output"
    grep -q 'no need to reconnect' "$TMP/path-output"
    grep -q 'PATH configured in:' "$TMP/path-output"
    HOME="$test_home" bash --noprofile --rcfile "$test_home/.bashrc" -ic 'lazydb version --json' > "$TMP/launched" 2>/dev/null
    grep -q '1.2.3' "$TMP/launched"
    cp "$test_home/.bashrc" "$TMP/profile-before"
    HOME="$test_home" sh "$installer" >/dev/null
    cmp "$test_home/.bashrc" "$TMP/profile-before"
    HOME="$test_home" PATH="$test_home/.local/bin:$PATH" sh "$installer" > "$TMP/ready"
    grep -q 'Ready to use in this terminal: lazydb' "$TMP/ready"
    if grep -q 'no need to reconnect' "$TMP/ready"; then
        printf '%s\n' 'ready installation still requested activation' >&2
        exit 1
    fi
    # Preserve dotfile links, permissions, and unrelated content when updating.
    mv "$test_home/.bashrc" "$test_home/shell-config"
    printf '\n# user configuration\n' >> "$test_home/shell-config"
    chmod 600 "$test_home/shell-config"
    ln -s shell-config "$test_home/.bashrc"
    special_dir="$test_home/bin with 'quotes' and \$dollars"
    HOME="$test_home" sh "$installer" --install-dir "$special_dir" >/dev/null
    [ -L "$test_home/.bashrc" ]
    grep -q '# user configuration' "$test_home/shell-config"
    [ "$(grep -c '# >>> LazyDB installer >>>' "$test_home/.bashrc")" -eq 1 ]
    HOME="$test_home" EXPECTED_BIN="$special_dir/lazydb" bash --noprofile --rcfile "$test_home/.bashrc" -ic '[ "$(command -v lazydb)" = "$EXPECTED_BIN" ] && lazydb version --json' >/dev/null 2>&1
    python3 - "$test_home/shell-config" <<'PY'
import os, stat, sys
assert stat.S_IMODE(os.stat(sys.argv[1]).st_mode) == 0o600
PY
    cp "$test_home/.bashrc" "$TMP/profile-before"
    HOME="$test_home" sh "$installer" --no-modify-path > "$TMP/skipped"
    cmp "$test_home/.bashrc" "$TMP/profile-before"
    grep -q 'Shell configuration unchanged' "$TMP/skipped"
    # Unknown shells and malformed managed blocks must not overwrite dotfiles.
    HOME="$test_home" SHELL=/bin/unknown sh "$installer" > "$TMP/unknown"
    grep -q 'unknown shell' "$TMP/unknown"
    cmp "$test_home/.bashrc" "$TMP/profile-before"
    printf '# >>> LazyDB installer >>>\n' > "$test_home/.bashrc"
    HOME="$test_home" sh "$installer" > "$TMP/malformed" 2>&1
    grep -q 'PATH setup needs attention' "$TMP/malformed"
    [ "$(wc -l < "$test_home/.bashrc")" -eq 1 ]
    # A read-only profile must not turn a successful binary install into failure.
    cp "$TMP/profile-before" "$test_home/shell-config"
    chmod 400 "$test_home/shell-config"
    HOME="$test_home" sh "$installer" > "$TMP/read-only" 2>&1
    if [ ! -w "$test_home/shell-config" ]; then
        grep -q 'PATH setup needs attention' "$TMP/read-only"
    fi
    chmod 600 "$test_home/shell-config"
done
# Shell-specific paths stay entirely inside the fixture home.
HOME="$TMP/home" SHELL=/bin/zsh ZDOTDIR="$TMP/zsh" sh "$TMP/pages/install.sh" >/dev/null
[ -f "$TMP/zsh/.zshrc" ]
HOME="$TMP/home" SHELL=/bin/fish XDG_CONFIG_HOME="$TMP/fish-config" sh "$TMP/pages/install.sh" >/dev/null
grep -q 'fish_add_path --path --' "$TMP/fish-config/fish/config.fish"
# Detect a command that shadows the new installation without removing it.
cp "$TMP/assets/lazydb" "$TMP/bin/lazydb"
HOME="$TMP/home" sh "$TMP/pages/install.sh" > "$TMP/shadowed"
grep -q 'another LazyDB takes precedence' "$TMP/shadowed"
[ -f "$TMP/bin/lazydb" ]
for mutation in missing extra; do
    python3 - "$TMP/server/channels/stable.json" "$mutation" <<'PY'
import json, sys
path, mutation = sys.argv[1:]
with open(path) as stream:
    data = json.load(stream)
if mutation == 'missing':
    del data['assets']['x86_64-unknown-linux-gnu']
else:
    data['assets']['unsupported-target'] = next(iter(data['assets'].values()))
with open(path, 'w') as stream:
    json.dump(data, stream)
PY
    for installer in "$TMP/pages/install.sh" "$ROOT/install.sh"; do
        if HOME="$TMP/home" XDG_DATA_HOME="$TMP/root-data" sh "$installer" --install-dir "$TMP/install" >"$TMP/error" 2>&1; then
            printf 'installer accepted %s target set\n' "$mutation" >&2
            exit 1
        fi
        grep -q 'manifest target set mismatch' "$TMP/error"
    done
    python3 "$ROOT/scripts/release/generate-channel-manifest.py" stable 1.2.3 2026-09-05T00:00:00Z "$TMP/server/assets" "$TMP/server/channels/stable.json"
done
chmod 000 "$LAZYDB_CONFIG_HOME/releases/1.2.3/lazydb"
if HOME="$TMP/home" sh "$TMP/pages/install.sh" > "$TMP/broken" 2>&1; then
    printf '%s\n' 'installer accepted an unusable final executable' >&2
    exit 1
fi
grep -q 'installed executable failed version check' "$TMP/broken"
chmod 755 "$LAZYDB_CONFIG_HOME/releases/1.2.3/lazydb"
printf '%s\n' 'installer tests: ok'
