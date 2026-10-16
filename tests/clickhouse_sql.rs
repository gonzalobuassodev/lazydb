use lazydb::sql::{
    SqlDialect, SqlRisk, SqlRiskAggregate, classify_sql, classify_statement_kind, scan_statements,
};

#[test]
fn clickhouse_native_query_syntax_is_parsed_and_select_is_read_only() {
    let dialect = SqlDialect::ClickHouse;
    for sql in [
        "SELECT number FROM numbers(5) ARRAY JOIN items AS item",
        "SELECT number FROM numbers(5) PREWHERE number > 0 LIMIT 1 BY number",
        "SELECT arrayMap(x -> x + 1, [1, 2])",
    ] {
        let risk = classify_sql(sql, dialect);
        assert_eq!(risk.risks, vec![SqlRisk::ReadOnly], "{sql}");
    }
}

#[test]
fn clickhouse_unknown_settings_and_mutations_fail_closed() {
    assert_eq!(
        classify_sql("SELECT 1 SETTINGS max_threads = 2", SqlDialect::ClickHouse).risks,
        vec![SqlRisk::Unknown]
    );
    assert_eq!(
        classify_sql(
            "ALTER TABLE events DELETE WHERE id = 1",
            SqlDialect::ClickHouse
        )
        .risks,
        vec![SqlRisk::Unknown]
    );
    assert_eq!(
        classify_statement_kind(
            "INSERT INTO events FORMAT JSONEachRow",
            SqlDialect::ClickHouse
        ),
        lazydb::sql::SqlStatementKind::Dml
    );
}

#[test]
fn clickhouse_admin_and_async_mutation_statements_fail_closed() {
    for sql in [
        "OPTIMIZE TABLE events FINAL",
        "KILL QUERY WHERE query_id = 'q1'",
        "SYSTEM DROP DNS CACHE",
        "GRANT SELECT ON db.* TO user1",
        "REVOKE SELECT ON db.* FROM user1",
        "SET max_threads = 2",
    ] {
        assert_eq!(
            classify_sql(sql, SqlDialect::ClickHouse).risks,
            vec![SqlRisk::Unknown],
            "{sql}"
        );
    }
}

#[test]
fn clickhouse_statement_scanner_handles_backticks_comments_and_escaped_strings() {
    let sql = "SELECT `semi;colon`, 'escaped\\';still' -- ; ignored\n; SELECT 2";
    let statements = scan_statements(sql, SqlDialect::ClickHouse);
    assert_eq!(statements.len(), 2);
    assert_eq!(
        &sql[statements[0].start..statements[0].end],
        "SELECT `semi;colon`, 'escaped\\';still' -- ; ignored\n;"
    );
    assert_eq!(&sql[statements[1].start..statements[1].end], "SELECT 2");
}

#[test]
fn clickhouse_multi_statement_risk_remains_explicit() {
    let analysis = classify_sql("SELECT 1; SELECT 2", SqlDialect::ClickHouse);
    assert_eq!(analysis.statement_count, 2);
    assert_eq!(analysis.aggregate, SqlRiskAggregate::MultiStatement);
}
