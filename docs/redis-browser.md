# Redis-Compatible Key/Value Browser

This browser is available for Redis and Valkey profiles. The user-facing driver
and connection identity remain distinct (`redis://`/`rediss://` versus
`valkey://`/`valkeys://`), while key discovery, previews, and mutations use the
shared Redis-compatible RESP adapter. Valkey-specific Cluster/Sentinel routing
and module types are not implemented.

## Loading modes

The browser initially loads Redis keys progressively. `SCAN COUNT` is only a
server hint; an empty response with a non-zero cursor is not complete. Results
are de-duplicated by raw key bytes.

Small keyspaces remain in memory. Large keyspaces may use a temporary SQLite
index so that key pages and prefix tree pages stay bounded. The index is local,
temporary, and is removed when its owner closes.

## Search and paging

Local find searches keys already received by the current browser. Redis MATCH
patterns control a new server-side scan and start a new scan generation.

Value previews are typed and bounded. Strings use byte ranges; Hashes and Sets
use cursors; Lists and Sorted Sets use ranges. A page can be partial and can
have a next position. The browser preserves raw bytes and only escapes them at
display time.

## Value preview formats

The selected Redis key displays its complete path, Redis type, Redis memory
usage when `MEMORY USAGE` is available, and TTL. Size labels use binary units
(`B`, `KB`, `MB`, and `GB`); the loaded page byte count is not presented as
the size of the complete key. TTL labels use compact units such as `55m30s`,
`850ms`, `∞`, `Missing`, or `—`.

The value area uses the same read-only Vim editor session as other LazyDB text
views, including normal-mode navigation, search, selection, mouse selection,
and editor scrollbars. `h` and `l` therefore move the Vim cursor when Preview
is focused; pane focus uses the normal pane-focus commands.

The format label can be clicked or opened from Preview with `Space f`. RAW, JSON, YAML, Table
projection, and Hex are supported views. JSON and YAML are formatted from
validated input; invalid structured input falls back to the raw view. Java,
PHP, Pickle, and schema-less Protobuf decoders are available in the
value-preview library and preserve explicit invalid/incomplete statuses.

Automatic selection validates JSON and strong serialization signatures before
selection, uses RAW for normal UTF-8, Hex for unknown binary, and Table for
Redis collections. The preview is read-only: decoded JSON/YAML is a display
projection and is not automatically encoded back to original serialized bytes.

Redis scans and value pages are not snapshots. A key may disappear or change
type between metadata and value reads. Old responses are discarded after a
database switch, refresh, tab close, or connection generation change.

Redis connections use a shared reconnecting connection manager. If a SCAN
request encounters a dropped transport such as `broken pipe`, the manager
reconnects using the original profile, credentials, TLS mode, and database;
the SCAN itself may be issued once more with the same cursor and pattern.
Reconnect attempts are bounded. If the server remains unavailable, the tab
shows the normal failed state and `r` starts a fresh scan after the next
connection attempt. Redis writes are not automatically replayed when their
response is lost, because the server may already have applied the command.

## Navigation and panes

Opening a Redis database from Explorer focuses the Keys pane. As soon as the
first non-empty scan batch is available, the first top-level tree node is
selected automatically. Later scan batches preserve the current selection.

Redis tabs are labeled `Redis @connection-name`; the name belongs to the tab's
connection profile rather than to the currently active connection.

Press `?` or `F1` to open contextual help. The help panel shows only shortcuts
that apply to the focused Redis area; its heading distinguishes Redis Keys,
text Value, Table Value, and key search states. In text Value browse mode, `?`
opens help; while a search or key-filter input is active, `?` remains text input.
Use `j`/`k` to move through Keys, `o` or `Enter` to expand/collapse a folder,
and `h`/`Left` to collapse or move to the parent, or `l`/`Right` to expand or
enter the first child. In Keys, `d` deletes the selected complete key and `y`
copies its complete name; folders cannot be deleted or copied. `/` filters all
keys already loaded for this database, including keys inside collapsed folders;
Enter confirms the filter and Esc restores the previous selection. The filter
does not start a new SCAN and is explicitly limited to the loaded client-side
snapshot. In Preview, the read-only Vim buffer supports `hjkl`, `w`/`b`/`e`,
`0`/`$`, `gg`/`G`, counts, Visual mode (`v`/`V`), `/` search, `n`/`N`,
and `yy`/Visual `y` copying. `Ctrl-d`/`Ctrl-u` move half a page and
PageUp/PageDown move a full page while keeping the text cursor visible. `Space
f` cycles format, `Space w` toggles wrapping, and `Space l` loads the next
value page. Pane focus uses `Ctrl-w h/l` or Tab/BackTab; `h`/`l` in Preview
remain text motions. Keys folders use the same group icons, colors, and selection
behavior as Explorer; leaf keys use a key icon.

Deleting a folder first scans the database independently, deduplicates matching
raw-byte keys, and shows the discovered count before confirmation. Confirmation
deletes only that frozen list; keys created later are not included in the same
operation. Large deletes run in bounded batches and report deleted and already
missing keys separately. A cancelled operation does not roll back batches that
already completed.

Keys and Preview maintain independent vertical scroll positions. Scrollbars
appear only when the loaded content overflows the pane; scrolling a Preview
does not change the selected key or issue a new Redis request.

## Limits

The `[redis]` section in `settings.toml` controls scan hints, value page sizes,
memory/index thresholds, preview debounce, metadata cache capacity, and disk
limits. Limits constrain client retention and display; Redis COUNT does not
guarantee a response element count.
