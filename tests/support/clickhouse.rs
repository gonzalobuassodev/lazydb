pub fn test_database_name() -> String {
    format!("lazydb_test_{}", uuid::Uuid::new_v4().simple())
}

pub async fn create_test_database(
    connection: &lazydb::db::DatabaseConnection,
    database: &str,
) -> Result<(), lazydb::db::DatabaseError> {
    connection
        .execute(&format!(
            "CREATE DATABASE `{}`",
            database.replace('`', "``")
        ))
        .await
        .map(|_| ())
}

pub async fn drop_test_database(
    connection: &lazydb::db::DatabaseConnection,
    database: &str,
) -> Result<(), lazydb::db::DatabaseError> {
    connection
        .execute(&format!(
            "DROP DATABASE IF EXISTS `{}`",
            database.replace('`', "``")
        ))
        .await
        .map(|_| ())
}
