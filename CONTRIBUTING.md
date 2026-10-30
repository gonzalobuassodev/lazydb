# Contributing

LazyDB is in an early architecture-sensitive phase. Small, tested changes are
preferred over broad abstractions or controls for unfinished features.

## Branches and Pull Requests

- One branch carries one pull request, and merging deletes the branch. The
  repository deletes the head branch automatically, so never keep a second
  branch holding the same work: a squash or a cherry-pick leaves the original
  commits permanently unmerged and they turn into conflicts later.
- When `develop` moves, merge `develop` into your branch. Never cut a new branch
  for a change that already has one; a stale branch is a conflict nobody asked
  for.
- Rebase is the only merge method enabled. Squash and merge commits are off, so
  the commits a pull request advertises are the commits that land.
- File the issue first. A branch without an issue and without a pull request is
  work that disappears with the branch.
- Land small. A feature that arrives as a single commit of several thousand
  lines cannot be reviewed, and it is the change most likely to be re-cut.

## Local Checks

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

The Neovim frontend and its test suite are maintained in the standalone
[`yelog/lazydb.nvim`](https://github.com/yelog/lazydb.nvim) repository.

PostgreSQL and MySQL integration tests run only when configured:

```bash
LAZYDB_TEST_POSTGRES_URL='postgresql://user:password@localhost:5432/database' \
  cargo test --test postgres_adapter

LAZYDB_TEST_MYSQL_URL='mysql://user:password@localhost:3306/database' \
  cargo test --test mysql_adapter
```

The isolated Valkey 8.1.10 fixture (`valkey/valkey@sha256:081c2f5cb575efc901aa80ff9cdbd1ec6a301682fd35e1ebb4b0990a4a4a8507`) binds only to loopback port 6380 and uses a dedicated volume. Start and validate it with:

```bash
docker compose -f docker-compose.valkey.yml config --quiet
docker compose -f docker-compose.valkey.yml up -d --wait
LAZYDB_TEST_VALKEY_URL='valkey://127.0.0.1:6380/0' \
  cargo test --locked --test valkey_adapter -- --ignored --nocapture --test-threads=1
LAZYDB_TEST_REDIS_URL='redis://127.0.0.1:6380/0' \
  cargo test --locked --test redis_mutation -- --ignored --nocapture --test-threads=1
docker compose -f docker-compose.valkey.yml down --volumes
```

Only run these commands against the dedicated fixture: tests create uniquely
prefixed keys and must never run `FLUSHDB` or `FLUSHALL` against a user server.
The Valkey adapter suite uses the application `valkey://` URL; the existing
Redis mutation regression uses the same server through the Redis-compatible
`redis://` transport URL.

The PostgreSQL adapter mutation coverage is intentionally serialized and uses
unique, temporary schema/object names. Run it directly with:

```bash
LAZYDB_TEST_POSTGRES_URL='postgresql://user:password@localhost:5432/database' \
  cargo test --test postgres_adapter serialized_postgres_catalog_mutations_round_trip_catalog_definitions -- --exact --nocapture
```

The mutation test needs only ordinary privileges to create and drop objects in
the configured database. Database and role creation are not part of standard
CI: they require elevated PostgreSQL privileges (`CREATEDB` for databases and
`CREATEROLE` for roles). To exercise the optional role path, use a disposable
test database and a role with `CREATEROLE`, set
`LAZYDB_TEST_POSTGRES_URL`, and run the privilege-gated role test directly. Do
not grant `SUPERUSER`, `BYPASSRLS`, or unrestricted production access solely for
the test suite. The exact command is:

```bash
LAZYDB_TEST_POSTGRES_URL='postgresql://user:password@localhost:5432/database' \
  cargo test --test postgres_adapter privileged_role_mutation_is_gated_by_environment_and_createrole -- --exact --nocapture
```

Database creation is likewise privilege-gated by PostgreSQL's `CREATEDB` (or
superuser) privilege and is intentionally not required by the standard adapter
suite; use a disposable maintenance database if exercising database mutation
manually.

Never paste real credentials into issues, tests, command examples, snapshots, or
logs.

## Design Rules

- Keep SQLx types inside database adapter modules.
- Keep side effects out of `App::update`; return `Command` values instead.
- Every asynchronous result carries owner identity and generation.
- Never return an uncertain connection to a pool.
- Never enable result editing without stable row identity and affected-row checks.
- Do not expose UI actions before they have an operational implementation.
- Database values are hostile terminal input until sanitized.
- Any user-facing configuration addition, removal, rename, or default change
  must update `config/default.toml`, configuration tests, and the relevant
  documentation in the same change. The embedded default file is the
  authoritative source of runtime defaults.
- Shortcut changes must update the shared shortcut catalog and
  `docs/keybindings.md`; changes to preset or timeout behavior must also update
  `config/default.toml`.

See [docs/architecture.md](docs/architecture.md) and the approved
[product design](docs/plans/2026-08-24-lazydb-design.md) before changing module
boundaries.
