#!/bin/sh
set -eu

DEFAULT_CHANNEL=stable
REPO=${LAZYDB_INSTALL_REPO:-yelog/lazydb}
CHANNEL=${LAZYDB_CHANNEL:-$DEFAULT_CHANNEL}
VERSION=${LAZYDB_VERSION:-}
INSTALL_DIR=${LAZYDB_INSTALL_DIR:-"$HOME/.local/bin"}
BASE_URL=${LAZYDB_CHANNEL_BASE_URL:-https://lazydb.yelog.org/channels}
MCP_SETUP=${LAZYDB_MCP_SETUP:-auto}
MODIFY_PATH=1

usage() { printf '%s\n' 'Usage: install.sh [--channel stable|beta] [--version VERSION] [--install-dir PATH] [--mcp-setup auto|skip|ask] [--no-modify-path]'; }
die() { printf 'lazydb installer: %s\n' "$*" >&2; exit 1; }
download() {
    request_url=
    for request_arg do request_url=$request_arg; done
    if [ -n "${LAZYDB_INSTALL_TEST_LOG:-}" ]; then printf '%s\n' "$request_url" >> "$LAZYDB_INSTALL_TEST_LOG"; fi
    curl "$@"
}
while [ "$#" -gt 0 ]; do
    case "$1" in
        --channel) [ "$#" -gt 1 ] || die '--channel needs a value'; CHANNEL=$2; shift 2 ;;
        --version) [ "$#" -gt 1 ] || die '--version needs a value'; VERSION=$2; shift 2 ;;
        --install-dir) [ "$#" -gt 1 ] || die '--install-dir needs a value'; INSTALL_DIR=$2; shift 2 ;;
        --mcp-setup) [ "$#" -gt 1 ] || die '--mcp-setup needs a value'; MCP_SETUP=$2; shift 2 ;;
        --no-modify-path) MODIFY_PATH=0; shift ;;
        --help) usage; exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
done
case "$MCP_SETUP" in auto|skip|ask) ;; *) die "invalid MCP setup mode: $MCP_SETUP" ;; esac
case "$CHANNEL" in stable|beta) ;; *) die "invalid channel: $CHANNEL" ;; esac
[ -z "${LAZYDB_CHANNEL_LOCKED:-}" ] || [ "$CHANNEL" = "$LAZYDB_CHANNEL_LOCKED" ] || die "channel is fixed to $LAZYDB_CHANNEL_LOCKED"
[ -n "${HOME:-}" ] || die 'HOME is required'

OS=$(uname -s); ARCH=$(uname -m)
case "$OS:$ARCH" in
    Darwin:x86_64) TARGET=x86_64-apple-darwin ;;
    Darwin:arm64|Darwin:aarch64) TARGET=aarch64-apple-darwin ;;
    Linux:x86_64|Linux:amd64) TARGET=x86_64-unknown-linux-gnu ;;
    Linux:aarch64|Linux:arm64) TARGET=aarch64-unknown-linux-gnu ;;
    *) die "unsupported platform: $OS/$ARCH" ;;
esac

for tool in curl tar awk cat cp mkdir mv ln chmod; do command -v "$tool" >/dev/null 2>&1 || die "$tool is required"; done
command -v python3 >/dev/null 2>&1 || die 'python3 is required'
python3 -c 'import lzma; lzma.LZMADecompressor(format=lzma.FORMAT_XZ)' >/dev/null 2>&1 \
    || die 'python3 cannot decode XZ archives; use a Python 3 installation with working lzma support. Homebrew is not required.'
INSTALL_DIR=$(python3 -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "$INSTALL_DIR")
if command -v sha256sum >/dev/null 2>&1; then HASH=sha256sum; else command -v shasum >/dev/null 2>&1 || die 'sha256sum or shasum is required'; HASH='shasum -a 256'; fi

if [ -n "${LAZYDB_CONFIG_HOME:-}" ]; then
    DATA_HOME=$LAZYDB_CONFIG_HOME
elif [ -f "$HOME/lazydb/connections.toml" ] || [ -f "$HOME/lazydb/credential.key" ] || [ -f "$HOME/lazydb/settings.toml" ] || [ -f "$HOME/lazydb/workspace.toml" ] || [ -f "$HOME/lazydb/install.json" ] || [ -d "$HOME/lazydb/releases" ] || [ -d "$HOME/lazydb/sql" ]; then
    DATA_HOME="$HOME/lazydb"
elif [ -f "$HOME/.config/lazydb/connections.toml" ] || [ -f "$HOME/.config/lazydb/credential.key" ] || [ -f "$HOME/.config/lazydb/settings.toml" ] || [ -f "$HOME/.config/lazydb/workspace.toml" ] || [ -f "$HOME/.config/lazydb/install.json" ] || [ -d "$HOME/.config/lazydb/releases" ] || [ -d "$HOME/.config/lazydb/sql" ]; then
    DATA_HOME="$HOME/.config/lazydb"
elif [ -f "$HOME/.local/share/lazydb/connections.toml" ] || [ -f "$HOME/.local/share/lazydb/credential.key" ] || [ -f "$HOME/.local/share/lazydb/settings.toml" ] || [ -f "$HOME/.local/share/lazydb/workspace.toml" ] || [ -f "$HOME/.local/share/lazydb/install.json" ] || [ -d "$HOME/.local/share/lazydb/releases" ] || [ -d "$HOME/.local/share/lazydb/sql" ]; then
    DATA_HOME="$HOME/.local/share/lazydb"
else
    DATA_HOME="$HOME/lazydb"
fi
RELEASES=$DATA_HOME/releases
TMP=$(mktemp -d "${TMPDIR:-/tmp}/lazydb-install.XXXXXX")
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT HUP INT TERM

mkdir -p "$DATA_HOME"
LOCK="$DATA_HOME/.install.lock"
LOCK_OWNER="lazydb-install-$$-$PPID"
acquire_lock() {
    if mkdir "$LOCK" 2>/dev/null; then
        printf '%s\n' "$$" > "$LOCK/pid"
        printf '%s\n' "$LOCK_OWNER" > "$LOCK/owner"
        return
    fi
    pid=$(cat "$LOCK/pid" 2>/dev/null || true)
    case "$pid" in
        ''|*[!0-9]*) die 'another installation may be running (lock owner is unknown)' ;;
        *) kill -0 "$pid" 2>/dev/null && die "another installation is running (pid $pid)"; die "stale installer lock found (pid $pid); remove $LOCK manually after verifying no installer is running" ;;
    esac
}
release_lock() {
    owner=$(cat "$LOCK/owner" 2>/dev/null || true)
    [ "$owner" = "$LOCK_OWNER" ] || return 0
    [ "$(cat "$LOCK/pid" 2>/dev/null || true)" = "$$" ] || return 0
    rm -f "$LOCK/owner" "$LOCK/pid"
    rmdir "$LOCK" 2>/dev/null || true
}
trap 'release_lock; cleanup' EXIT HUP INT TERM
acquire_lock

MANIFEST="$TMP/manifest.json"
if download --location --proto '=https' --tlsv1.2 --max-time 60 -sS -w '%{http_code}' -o "$MANIFEST" "$BASE_URL/v2/$CHANNEL.json" > "$TMP/manifest-status"; then
    MANIFEST_STATUS=$(cat "$TMP/manifest-status")
else
    die 'manifest download failed'
fi
if [ "$MANIFEST_STATUS" = 404 ]; then
    download --fail --location --proto '=https' --tlsv1.2 --max-time 60 -o "$MANIFEST" "$BASE_URL/$CHANNEL.json" || die 'legacy manifest download failed'
    MANIFEST_SCHEMA=1
elif [ "$MANIFEST_STATUS" = 200 ]; then MANIFEST_SCHEMA=2
else die "manifest request returned HTTP $MANIFEST_STATUS"
fi
python3 - "$MANIFEST" "$CHANNEL" "$TARGET" "$VERSION" "$MANIFEST_SCHEMA" "$TMP/metadata" <<'PY'
import json, re, sys
from urllib.parse import urlparse
path, channel, target, requested, expected_schema, output = sys.argv[1:]
try:
    data = json.load(open(path, encoding='utf-8'))
    if data.get('schema') != int(expected_schema) or data.get('product') != 'lazydb' or data.get('channel') != channel:
        raise ValueError('manifest identity mismatch')
    version = data['version']
    if not re.fullmatch(r'(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-beta\.[1-9][0-9]*)?', version): raise ValueError('invalid version')
    if (channel == 'beta') != ('-beta.' in version) or data.get('tag') != 'v' + version or bool(data.get('prerelease')) != (channel == 'beta'): raise ValueError('manifest version mismatch')
    if requested and requested != version: raise ValueError('requested version is not the channel version')
    supported = {'x86_64-apple-darwin', 'aarch64-apple-darwin', 'x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu', 'x86_64-pc-windows-msvc'}
    if set(data.get('assets', {})) != supported: raise ValueError('manifest target set mismatch')
    asset = data['assets'][target]
    if target.endswith('-linux-gnu'):
        if expected_schema == '2':
            libc = asset['requirements']['libc']
            if libc.get('family') != 'glibc' or not re.fullmatch(r'[0-9]+\.[0-9]+', libc.get('min_version', '')):
                raise ValueError('invalid Linux libc requirement')
            import subprocess
            glibc_version = subprocess.check_output(['getconf', 'GNU_LIBC_VERSION'], text=True).split()[-1]
            try:
                have = tuple(int(part) for part in glibc_version.split('.'))
                need = tuple(int(part) for part in libc['min_version'].split('.'))
            except ValueError as error: raise ValueError('invalid glibc version returned by getconf') from error
            if have < need: raise ValueError('Linux runtime requires glibc %s, this system has %s' % (libc['min_version'], glibc_version))
        elif 'requirements' in asset:
            raise ValueError('unexpected asset requirements')
    elif 'requirements' in asset:
        raise ValueError('unexpected asset requirements')
    url = asset['url']; parsed = urlparse(url)
    if parsed.scheme != 'https' or parsed.netloc not in {'github.com', 'lazydb.yelog.org'} or not re.fullmatch(r'[0-9a-f]{64}', asset['sha256']): raise ValueError('invalid asset')
    name = url.rsplit('/', 1)[-1]
    if name != 'lazydb_%s_%s.tar.xz' % (version, target): raise ValueError('invalid asset name')
    with open(output, 'w', encoding='utf-8') as stream: stream.write('%s\n%s\n%s\n' % (version, url, asset['sha256']))
except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as exc:
    print('manifest: %s' % exc, file=sys.stderr); raise SystemExit(1)
PY
[ -s "$TMP/metadata" ] || die 'invalid channel manifest'
RELEASE_VERSION=$(awk 'NR == 1 { print; exit }' "$TMP/metadata")
ARCHIVE_URL=$(awk 'NR == 2 { print; exit }' "$TMP/metadata")
EXPECTED=$(awk 'NR == 3 { print; exit }' "$TMP/metadata")
ARCHIVE_NAME="lazydb_${RELEASE_VERSION}_${TARGET}.tar.xz"
download --fail --location --proto '=https' --tlsv1.2 --max-time 60 -o "$TMP/archive.tar.xz" "$ARCHIVE_URL" || die 'archive download failed'
ACTUAL=$($HASH "$TMP/archive.tar.xz" | awk '{print $1}')
[ "$ACTUAL" = "$EXPECTED" ] || die 'checksum mismatch'

mkdir -p "$TMP/unpack"
python3 - "$TMP/archive.tar.xz" "$TMP/unpack" "$ARCHIVE_NAME" <<'PY'
import sys, tarfile
archive, output, expected = sys.argv[1:]
with tarfile.open(archive, 'r:xz') as tar:
    members = tar.getmembers(); root = expected[:-7]; names = set()
    for member in members:
        name = member.name
        if name.startswith('/') or name in names or any(part in ('', '..') for part in name.split('/')) or (name != root and not name.startswith(root + '/')): raise SystemExit('unsafe archive entry: ' + name)
        if member.issym() or member.islnk() or not (member.isdir() or member.isreg()): raise SystemExit('unsupported archive entry: ' + name)
        names.add(name)
    binary = root + '/lazydb'
    if binary not in names or not any(tarinfo.name == binary and tarinfo.isreg() for tarinfo in members): raise SystemExit('archive does not contain the expected executable')
    tar.extractall(output)
PY
chmod 0755 "$TMP/unpack"/*/lazydb
STAGED="$TMP/release"; mkdir -p "$STAGED"; cp -R "$TMP/unpack"/*/. "$STAGED/"
"$STAGED/lazydb" version --json > "$TMP/version.json" || die 'staged binary failed version check'
python3 - "$TMP/version.json" "$RELEASE_VERSION" <<'PY'
import json,sys
data=json.load(open(sys.argv[1], encoding='utf-8'))
if data.get('version') != sys.argv[2]: raise SystemExit('binary reported version %r' % data.get('version'))
PY
DEST="$RELEASES/$RELEASE_VERSION"
FIRST_INSTALL=1
if [ -e "$DATA_HOME/install.json" ] || [ -e "$DATA_HOME/current" ] || [ -e "$INSTALL_DIR/lazydb" ]; then
    FIRST_INSTALL=0
fi
if [ ! -e "$DEST" ]; then
    mkdir -p "$RELEASES"
    python3 - "$STAGED" "$RELEASE_VERSION" "$TARGET" "$DATA_HOME/current" <<'PY'
import hashlib, json, os, stat, sys, tempfile
root, version, target, current = sys.argv[1:]
entries = []
for name in sorted(os.listdir(root)):
    path = os.path.join(root, name)
    mode = os.lstat(path).st_mode
    if stat.S_ISLNK(mode) or not stat.S_ISREG(mode): raise SystemExit('staged release contains an unexpected file')
    with open(path, 'rb') as stream: digest = hashlib.sha256(stream.read()).hexdigest()
    entries.append({'path': name, 'sha256': digest, 'size': os.path.getsize(path)})
binary = next((item for item in entries if item['path'] == 'lazydb'), None)
if binary is None: raise SystemExit('staged release binary is missing')
previous = None
if os.path.islink(current):
    resolved = os.path.realpath(current)
    releases = os.path.realpath(os.path.join(os.path.dirname(current), 'releases'))
    if os.path.dirname(resolved) == releases: previous = os.path.basename(resolved)
marker = {'schema': 1, 'product': 'lazydb', 'version': version, 'target': target,
          'binary_sha256': binary['sha256'], 'previous_version': previous, 'files': entries}
fd, path = tempfile.mkstemp(prefix='.release.json.', dir=root)
with os.fdopen(fd, 'w', encoding='utf-8') as stream:
    json.dump(marker, stream, sort_keys=True, indent=2); stream.write('\n'); stream.flush(); os.fsync(stream.fileno())
os.replace(path, os.path.join(root, '.release.json'))
PY
    mv "$STAGED" "$DEST"
fi
python3 - "$DEST" "$DATA_HOME/current" <<'PY'
import os, sys
temporary = sys.argv[2] + '.new.' + str(os.getpid())
try:
    os.symlink(sys.argv[1], temporary)
    os.replace(temporary, sys.argv[2])
finally:
    if os.path.lexists(temporary):
        os.unlink(temporary)
PY
mkdir -p "$INSTALL_DIR"; ln -sfn "$DATA_HOME/current/lazydb" "$INSTALL_DIR/lazydb"
"$INSTALL_DIR/lazydb" version --json > "$TMP/version.json" || die 'installed executable failed version check'
python3 - "$TMP/version.json" "$RELEASE_VERSION" <<'PY'
import json, sys
if json.load(open(sys.argv[1], encoding='utf-8')).get('version') != sys.argv[2]:
    raise SystemExit('installed executable reported an unexpected version')
PY
STATE="$DATA_HOME/install.json"
python3 - "$STATE" "$TMP/state" "$CHANNEL" "$RELEASE_VERSION" "$TARGET" "$INSTALL_DIR/lazydb" <<'PY'
import json, os, sys, tempfile
state = {'schema': 1, 'product': 'lazydb', 'manager': 'native', 'channel': sys.argv[3], 'version': sys.argv[4], 'target': sys.argv[5], 'path': sys.argv[6]}
fd, path = tempfile.mkstemp(prefix='.install.json.', dir=os.path.dirname(sys.argv[1]))
with os.fdopen(fd, 'w', encoding='utf-8') as stream: json.dump(state, stream, indent=2); stream.write('\n'); stream.flush(); os.fsync(stream.fileno())
os.replace(path, sys.argv[1])
PY
printf 'lazydb %s installed (%s)\n' "$RELEASE_VERSION" "$CHANNEL"
# This child process can configure future shells, but cannot change the caller's PATH.
SHELL_PROFILE_RECORD="$TMP/shell-profile"
python3 - "$INSTALL_DIR" "$OS" "$MODIFY_PATH" "$SHELL_PROFILE_RECORD" <<'PY'
import os, shlex, shutil, sys
from pathlib import Path

directory, system, modify, record_path = sys.argv[1:]
shell = Path(os.environ.get('SHELL', '')).name
home = Path.home()
profile = None
if shell == 'bash':
    profile = home / ('.bash_profile' if system == 'Darwin' else '.bashrc')
elif shell == 'zsh':
    profile = Path(os.environ.get('ZDOTDIR') or home) / '.zshrc'
elif shell == 'fish':
    profile = Path(os.environ.get('XDG_CONFIG_HOME') or home / '.config') / 'fish/config.fish'
elif shell in ('sh', 'dash', 'ash'):
    profile = home / '.profile'

quoted = shlex.quote(directory)
activate = 'export PATH=' + quoted + ':"$PATH"'
body = 'case ":$PATH:" in\n    *:' + quoted + ':*) ;;\n    *) ' + activate + ' ;;\nesac\n'
if shell == 'fish':
    quoted = "'" + directory.replace('\\', '\\\\').replace("'", "\\'") + "'"
    activate = 'fish_add_path --path -- ' + quoted
    body = activate + '\n'

visible = shutil.which('lazydb')
executable = str(Path(directory) / 'lazydb')
ready = bool(visible and os.path.samefile(visible, executable))
print('Executable: ' + executable)
if visible and not ready:
    print('WARNING: another LazyDB takes precedence on PATH: ' + visible)

configured = False
if modify == '1' and profile is not None:
    begin, end = '# >>> LazyDB installer >>>', '# <<< LazyDB installer <<<'
    block = begin + '\n' + body + end + '\n'
    try:
        # Follow dotfile symlinks and write in place to preserve their permissions.
        target = profile.resolve()
        old = target.read_text() if target.exists() else ''
        lines = old.splitlines(keepends=True)
        starts = [i for i, line in enumerate(lines) if line.rstrip('\r\n') == begin]
        ends = [i for i, line in enumerate(lines) if line.rstrip('\r\n') == end]
        if starts or ends:
            if len(starts) != 1 or len(ends) != 1 or starts[0] >= ends[0]:
                raise ValueError('incomplete or duplicate LazyDB PATH block; left unchanged')
            new = ''.join(lines[:starts[0]]) + block + ''.join(lines[ends[0] + 1:])
        else:
            new = old + ('\n' if old else '') + block
        if new != old:
            target.parent.mkdir(parents=True, exist_ok=True)
            with target.open('w') as stream:
                stream.write(new)
        configured = True
        import hashlib
        block_hash = hashlib.sha256(block.encode()).hexdigest()
        Path(record_path).write_text(str(profile) + '\n' + block_hash + '\n')
        print('PATH configured in: ' + str(profile))
    except (OSError, ValueError) as error:
        print('WARNING: PATH setup needs attention: ' + str(error), file=sys.stderr)
elif modify != '1':
    print('Shell configuration unchanged (--no-modify-path).')
else:
    print('WARNING: unknown shell; configure PATH manually.')

if configured:
    print('Future shells that load this file will include the installation directory on PATH.')
elif not ready:
    print('Add the installation directory to your shell startup configuration for future sessions.')
PY
python3 - "$STATE" "$SHELL_PROFILE_RECORD" <<'PY'
import json, os, sys, tempfile
state_path, record_path = sys.argv[1:]
try:
    record = open(record_path, encoding='utf-8').read().splitlines()
    profile = record[0] if record else ''
    block_hash = record[1] if len(record) > 1 else ''
except OSError:
    profile = ''
if profile:
    with open(state_path, encoding='utf-8') as stream:
        state = json.load(stream)
    profiles = state.get('shell_profiles', [])
    entry = {'path': profile, 'block_sha256': block_hash}
    if entry not in profiles:
        profiles.append(entry)
    state['shell_profiles'] = profiles
    fd, temporary = tempfile.mkstemp(prefix='.install.json.', dir=os.path.dirname(state_path))
    with os.fdopen(fd, 'w', encoding='utf-8') as stream:
        json.dump(state, stream, indent=2)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, state_path)
PY
printf '%s\n' 'Configure LazyDB MCP for Claude Code, Codex, or OpenCode; you can choose a user-level or project-level configuration.'
if [ "$MCP_SETUP" != skip ]; then
    mcp_setup_interactive() {
        if ! [ -r /dev/tty ] || ! [ -w /dev/tty ]; then
            printf '%s\n' 'MCP setup was skipped because no interactive terminal is available.' >&2
            printf '%s\n' 'Run `lazydb mcp setup` in the target project directory to configure it.' >&2
            return 0
        fi
        if [ "$MCP_SETUP" = ask ] || [ "$FIRST_INSTALL" = 1 ]; then
            printf '%s' 'Configure LazyDB MCP now? [y/N] ' > /dev/tty
            answer=
            IFS= read -r answer < /dev/tty || answer=
            answer=$(printf '%s' "$answer" | tr '[:upper:]' '[:lower:]' | awk '{$1=$1};1')
            case "$answer" in
                y|yes)
                    if "$INSTALL_DIR/lazydb" mcp setup </dev/tty >/dev/tty 2>&1; then
                        :
                    else
                        printf '%s\n' 'LazyDB is installed, but MCP setup did not complete.' > /dev/tty
                        printf '%s\n' "Run '$INSTALL_DIR/lazydb mcp setup' to retry." > /dev/tty
                    fi
                    ;;
            esac
        fi
    }
    mcp_setup_interactive
fi
python3 - "$INSTALL_DIR" <<'PY'
import os, shlex, shutil, sys
from pathlib import Path

directory = sys.argv[1]
shell = Path(os.environ.get('SHELL', '')).name
quoted = shlex.quote(directory)
activate = 'export PATH=' + quoted + ':"$PATH"'
if shell == 'fish':
    quoted = "'" + directory.replace('\\', '\\\\').replace("'", "\\'") + "'"
    activate = 'fish_add_path --path -- ' + quoted
visible = shutil.which('lazydb')
executable = str(Path(directory) / 'lazydb')
ready = bool(visible and os.path.samefile(visible, executable))
if ready:
    print('Ready to use in this terminal: lazydb')
elif shell in ('bash', 'zsh', 'fish', 'sh', 'dash', 'ash'):
    print('Run in your current terminal (no need to reconnect):\n  ' + activate + '\n  lazydb')
else:
    print('Run the executable directly: ' + shlex.quote(executable))
PY
