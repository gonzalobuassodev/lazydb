# Neovim Integration and SQL Language Server

LazyDB offers a Neovim terminal plugin and a standalone SQL/MyBatis language
server. Install the [LazyDB CLI](installation.md) first. The plugin starts the
TUI; the language server supplies editor diagnostics and completion.

## Neovim Integration

[`yelog/lazydb.nvim`](https://github.com/yelog/lazydb.nvim) runs one LazyDB
process per Neovim tab in a floating terminal. For `lazy.nvim`:

```lua
return {
  {
    "yelog/lazydb.nvim",
    cmd = { "LazyDB", "LazyDBToggle", "LazyDBHide", "LazyDBStop", "LazyDBRestart" },
    opts = {
      executable = "lazydb",
      window = { width = 0.92, height = 0.90, border = "rounded" },
    },
  },
}
```

Use `:checkhealth lazydb` to verify the executable and CLI API. The plugin
repository maintains native package installation instructions and the complete
command reference. See [external themes](configuration.md#external-theme-file)
for following Neovim colors.

## Start the Language Server

Configure your editor's LSP client to launch:

```sh
lazydb lsp --stdio --project . --dialect postgres
```

The project directory is resolved from the language server's working directory;
use an explicit path when the editor starts it elsewhere. The server provides
offline SQL/MyBatis syntax diagnostics and completion, plus profile-backed
catalog completion when a project/profile can be resolved. It does not execute
user SQL, perform database-backed diagnostics, or infer application-language
types. Standard output is reserved for framed JSON-RPC messages; logs go to
standard error.

## Catalog Completion

Completion replaces only the current identifier segment. Accepting `sys_user`
inserts `sys_user` rather than an automatically generated full path. After a
dot, completion navigates the next catalog level: `database.` lists schemas,
`database.schema.` lists tables/views, and `schema.` resolves inside the active
database, where the selected dialect supports those paths. Candidate details
show the database/schema source for ambiguous names; visible aliases such as
`u.` load their relation columns on demand.

The path syntax follows the profile dialect. SQL Server supports three-part
names, MySQL and MariaDB use a database-as-schema model, SQLite uses attached
database names such as `main`, and PostgreSQL does not present ordinary
cross-database paths. Oracle uses service/owner scope. Redis and Valkey use
key/value workflows rather than SQL identifier completion. Profile
`catalog_scope` and database permissions remain the visibility boundary.

Without a usable profile, completion falls back to offline SQL candidates.
If catalog candidates are missing, check the selected project's profile access,
catalog scope, and database grants in [Configuration](configuration.md).

## Implementation Notes

The sections below preserve parser, protocol, and MyBatis design decisions.
They explain implementation boundaries and the original rollout stages.

### SQL Parser Locations

The pinned `sqlparser` version is `0.62.0`. Its public `ParserError` has three
variants, including a string-based `ParserError(String)` variant. It does not
carry a public structured source span. Parser messages currently contain line
and column wording for common syntax errors, but that message format is not a
stable source-location API.

The tokenizer does expose structured `Location` and `Span` values. Parser
errors do not reliably do so: for example, an incomplete predicate ending at
EOF is reported as `Expected: an expression, found: EOF` with no line or
column. The future diagnostic implementation must therefore use this order:

1. Tokenize first and use the structured tokenizer location when tokenization
   fails.
2. Parse the SQL and use a parser message only for its human-readable text;
   do not claim a token location unless a future parser version exposes a
   tested structured location.
3. If the parser cannot provide a reliable location, publish a diagnostic over the current SQL
   statement (or the mapped embedded unit), not a guessed token range.

The parser probe is covered by `tests/sql_parser_location_probe.rs`. A future
upgrade of `sqlparser` must rerun this probe before changing the diagnostic
adapter.

### Protocol Dependency Decision

Do not hand-roll JSON-RPC framing. Task 2 must select a maintained Rust LSP
protocol implementation after checking its current MSRV, Tokio integration,
stdio transport, cancellation, and license. The server stdout contract is
strict: only framed JSON-RPC messages may be written there; logs belong on
stderr.

### MyBatis XML Recovery Strategy

The first implementation will use a bounded, source-span-preserving scanner
for mapper XML rather than removing tags with a regular expression. It must
recognize XML comments, CDATA, entity references, CRUD statement boundaries,
and incomplete tags without fetching external DTDs or entities.

An XML parser may be used as an optional validation pass when it can recover
from incomplete input, but a strict parser cannot be the only source of SQL
units because completion must work while the user is typing. The scanner will
mark entity-decoded, parameter-replaced, synthesized, and unknown dynamic
segments separately so that unsafe edits and untrustworthy diagnostics can be
suppressed.

The initial XML fixture matrix is:

- multiple `select`, `insert`, `update`, and `delete` statements;
- ordinary text and CDATA SQL;
- `&lt;`, `&gt;`, `&amp;`, and numeric character references;
- `#{parameter}` and `${dynamic}` expressions;
- comments, DOCTYPE declarations, incomplete tags, and malformed attributes;
- non-SQL mapper elements and attributes containing SQL-looking words.

Dynamic tags and `<include>` are not considered statically valid merely
because their text can be concatenated. Until their source mapping is
implemented, affected units must provide only safe local completion and must
suppress whole-unit syntax diagnostics.

### M1/M2 Diagnostic Boundary

The first release provides offline SQL syntax diagnostics for plain SQL and
trusted static MyBatis SQL. It does not execute SQL, query the database for
validation, infer Java/OGNL types, or prove that every dynamic SQL branch is
valid.

### Current Implementation Boundary

The stdio server, document lifecycle, UTF-16 conversion, offline completion,
syntax diagnostics, static MyBatis extraction, conservative dynamic-tag
handling, and profile-backed catalog completion are implemented and covered by
protocol/integration tests. Catalog completion is loaded lazily through the
selected project/profile scope; if no usable profile is available, the server
falls back to offline SQL completion. The language server never executes user
SQL or performs database-backed diagnostics.
