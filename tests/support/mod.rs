#[allow(dead_code)]
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

#[allow(dead_code)]
pub fn mongodb_test_url() -> Option<String> {
    match std::env::var("LAZYDB_TEST_MONGODB_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        Ok(_) | Err(std::env::VarError::NotPresent) => {
            if std::env::var_os("LAZYDB_REQUIRE_DATABASE_TESTS").is_some() {
                panic!("LAZYDB_TEST_MONGODB_URL is required for MongoDB integration tests");
            }
            eprintln!("SKIP: LAZYDB_TEST_MONGODB_URL is not set");
            None
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            panic!("LAZYDB_TEST_MONGODB_URL is not valid Unicode")
        }
    }
}

#[allow(dead_code)]
pub fn mongodb_standalone_url() -> Option<String> {
    match std::env::var("LAZYDB_TEST_MONGODB_STANDALONE_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        Ok(_) | Err(std::env::VarError::NotPresent) => {
            if std::env::var_os("LAZYDB_REQUIRE_DATABASE_TESTS").is_some() {
                panic!("LAZYDB_TEST_MONGODB_STANDALONE_URL is required for MongoDB topology tests");
            }
            eprintln!("SKIP: LAZYDB_TEST_MONGODB_STANDALONE_URL is not set");
            None
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            panic!("LAZYDB_TEST_MONGODB_STANDALONE_URL is not valid Unicode")
        }
    }
}

#[allow(dead_code)]
pub fn mongodb_principals_url() -> Option<String> {
    match std::env::var("LAZYDB_TEST_MONGODB_PRINCIPALS_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        Ok(_) | Err(std::env::VarError::NotPresent) => {
            if std::env::var_os("LAZYDB_REQUIRE_DATABASE_TESTS").is_some() {
                panic!(
                    "LAZYDB_TEST_MONGODB_PRINCIPALS_URL is required for MongoDB principal integration tests"
                );
            }
            eprintln!("SKIP: LAZYDB_TEST_MONGODB_PRINCIPALS_URL is not set");
            None
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            panic!("LAZYDB_TEST_MONGODB_PRINCIPALS_URL is not valid Unicode")
        }
    }
}
