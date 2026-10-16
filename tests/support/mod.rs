#[allow(dead_code)]
pub fn clickhouse_test_url() -> Option<String> {
    match std::env::var("LAZYDB_TEST_CLICKHOUSE_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        Ok(_) | Err(std::env::VarError::NotPresent) => {
            if std::env::var_os("LAZYDB_REQUIRE_DATABASE_TESTS").is_some() {
                panic!("LAZYDB_TEST_CLICKHOUSE_URL is required for ClickHouse integration tests");
            }
            eprintln!("SKIP: LAZYDB_TEST_CLICKHOUSE_URL is not set");
            None
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            panic!("LAZYDB_TEST_CLICKHOUSE_URL is not valid Unicode")
        }
    }
}

pub fn mariadb_test_url() -> Option<String> {
    match std::env::var("LAZYDB_TEST_MARIADB_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        Ok(_) | Err(std::env::VarError::NotPresent) => {
            if std::env::var_os("LAZYDB_REQUIRE_DATABASE_TESTS").is_some() {
                panic!("LAZYDB_TEST_MARIADB_URL is required for MariaDB integration tests");
            }
            eprintln!("SKIP: LAZYDB_TEST_MARIADB_URL is not set");
            None
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            panic!("LAZYDB_TEST_MARIADB_URL is not valid Unicode")
        }
    }
}
