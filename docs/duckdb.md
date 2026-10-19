# DuckDB

LazyDB connects directly to DuckDB through the bundled native engine. It supports local files and in-memory databases; it does not require a server process.

## Connect

Use a saved profile or pass a URL to the CLI:

```sh
lazydb connections add --name analytics --url 'duckdb:./analytics.duckdb?schema=main' --scope project
lazydb --profile analytics
```

An absolute path can use `duckdb:/data/analytics.duckdb`. A relative path is resolved from the project for project profiles. `duckdb::memory:` creates an in-memory database; it lasts only as long as the owning LazyDB process/session. Add `read_only=true` to open an existing file read-only. A missing read-only file is an error and is not created.

DuckDB uses an embedded file lock. Another process may be unable to open the same file concurrently, depending on its access mode. Close the other process or use a separate file. LazyDB console sessions share the database while keeping session-local temporary objects isolated.

## Catalog and data

Explorer reads the selected catalog and schema, tables, views, columns, and searchable objects. Data preview supports the relation tab's filters, ordering, and paging. DDL is read from DuckDB's native catalog. Attached databases and temporary objects have session-specific visibility; a temporary table is visible only in the console that created it.

Structured schema, table, view, sequence, and ART index creation is available. The table form creates and reloads typed PRIMARY KEY, UNIQUE, CHECK, and FOREIGN KEY constraints. DuckDB does not expose foreign key action metadata through its catalog, so the structured editor supports only MATCH SIMPLE with NO ACTION and rejects other action values. DEFERRABLE, INITIALLY DEFERRED, NOT VALID, and opaque constraint strings are rejected. Sequence forms expose start, increment, min/max, and cycle; DuckDB does not expose sequence type, cache, ownership, or comments in its sequence catalog, so unsupported draft values are rejected. Index forms support ordinary column keys and uniqueness; expression, descending, include, predicate, and tablespace fields are rejected. Index metadata comes from DuckDB's native catalog; DuckDB does not provide replayable index SQL through `duckdb_indexes().sql`. Table editing supports table and column rename, nullable column add, column drop, type change, default change, nullability change, and table/column comments through native DDL; dependency and constraint errors come from DuckDB without a table rebuild. Existing table constraints load into the typed form but constraint edits are gated until DuckDB can represent their definition losslessly. View editing supports rename, query replacement, and comments. Definitions and stale checks include native table/column/view comments, constraints, and index metadata. Index changes through the table editor are gated because its string representation is not a lossless structured contract. Each supported edit checks native metadata before applying. Table/view/sequence/index/type/schema drop plans are available; DuckDB itself rejects dropping a non-empty schema. Macro structured management is gated because macros share the function catalog identity; use SQL for macros and other DuckDB-specific objects. Relation grid editing supports primary-key tables with typed insert, update, and delete requests. LazyDB does not install extensions automatically.

DuckDB scalar, nested, temporal, and decimal values retain their native type name in result metadata. `TIME_NS` SELECT, WITH, and FROM-first results are converted through DuckDB's native VARCHAR cast before row materialization, preserving nanosecond values without using the pinned Rust row decoder's unsupported path. Duplicate or empty output column names in such a result are rejected explicitly.

## Transactions and headless access

MANUAL transactions execute on the console's existing DuckDB session. Commit and rollback apply to that session. Cancelling work interrupts the native DuckDB operation; an operation that cannot be confirmed is not reported as committed.

The agent/MCP service opens a connection per request. Use a file profile when state must persist across requests; `duckdb::memory:` does not persist across independent headless requests. Writes remain subject to the agent write policy and the profile's read-only setting.

The language server accepts `--dialect duckdb`; a selected DuckDB profile also supplies its dialect for SQL analysis and catalog completion.
