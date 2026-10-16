# ClickHouse Test Database

LazyDB's ClickHouse adapter targets the HTTP interface and is exercised against
ClickHouse `25.8.12` (LTS). The deterministic SQL fixture creates a small
table and restricted user inside `lazydb_test`; live catalog mutation tests
create uniquely prefixed objects and clean up their own database.

## Local service

Start the isolated bootstrap service, then apply the SQL fixture to create its
test account and grants:

```sh
docker compose -f docker-compose.clickhouse.yml up -d --wait
ruby -e 'File.read("tests/fixtures/clickhouse/init.sql").split(/;\s*(?=\S)/).each { |sql| next if sql.strip.empty?; system("curl", "--fail", "--silent", "--show-error", "--data-binary", sql, "http://127.0.0.1:18123/") or exit 1 }'
```

Set the URL and run the required adapter/capability suite:

```sh
export LAZYDB_TEST_CLICKHOUSE_URL=clickhouse://lazydb_test:lazydb_test@127.0.0.1:18123/lazydb_test
LAZYDB_REQUIRE_DATABASE_TESTS=1 cargo test --locked \
  --test clickhouse_adapter --test clickhouse_catalog_mutation \
  -- --nocapture --test-threads=1
```

The shared `clickhouse_adapter` tests verify that a missing URL is a visible
skip for ordinary local work and a failure when `LAZYDB_REQUIRE_DATABASE_TESTS=1`
is set. The CI database job sets the URL and that exact value for its live step.
Mock transport, catalog, policy, monitoring, principal,
MCP, and LSP tests do not need a running server.

## Scope and cleanup

The supplied compose file binds its HTTP port to `127.0.0.1:18123` and has no
host data volume. The initialized test user has administrative grants inside this
dedicated disposable server because catalog round-trip tests create uniquely
named databases. Test helpers clean up only UUID-prefixed objects they created.
Do not point this fixture at a production server or reuse an existing database
with unrelated data.

Stop and remove the compose service after local use:

```sh
docker compose -f docker-compose.clickhouse.yml down --volumes
```

The command removes only resources created by this compose project. Do not use
it with an overridden compose project name shared with another environment.
