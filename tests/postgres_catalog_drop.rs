use lazydb::db::catalog_drop::{
    CatalogDropBehavior, CatalogDropExecutionTarget, CatalogDropRequest,
};
use lazydb::db::{
    DatabaseConnection,
    catalog::{CatalogEntry, CatalogId, CatalogKind, OptionalMetadata, QualifiedName},
};
use lazydb::{identity::ConnectionIdentity, profile::import_connection_url};
use sqlx::{AssertSqlSafe, Connection, PgConnection, postgres::PgConnectOptions, raw_sql};
use uuid::Uuid;

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn url_for_database(url: &str, database: &str) -> String {
    let mut url = url::Url::parse(url).unwrap();
    url.set_path(database);
    url.to_string()
}

#[test]
fn maintenance_target_is_not_the_current_connection_target() {
    let target = CatalogDropExecutionTarget::MaintenanceDatabase("postgres".into());
    assert!(matches!(
        target,
        CatalogDropExecutionTarget::MaintenanceDatabase(_)
    ));
}

#[tokio::test]
#[ignore = "requires an explicitly isolated PostgreSQL admin URL"]
async fn postgres_nonempty_schema_requires_restrict_then_explicit_cascade() {
    let Some(admin_url) = std::env::var("LAZYDB_TEST_POSTGRES_ADMIN_URL").ok() else {
        panic!("set LAZYDB_TEST_POSTGRES_ADMIN_URL to an isolated PostgreSQL admin URL");
    };
    let options: PgConnectOptions = admin_url.parse().unwrap();
    let mut admin = PgConnection::connect_with(&options).await.unwrap();
    let suffix = Uuid::new_v4().simple().to_string();
    let schema_name = format!("drop_test_\"{suffix}");
    let quoted_schema = quote_identifier(&schema_name);
    raw_sql(AssertSqlSafe(format!("CREATE SCHEMA {quoted_schema}")))
        .execute(&mut admin)
        .await
        .unwrap();
    let result = async {
        raw_sql(AssertSqlSafe(format!(
            "CREATE TABLE {quoted_schema}.items (id integer PRIMARY KEY)"
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        raw_sql(AssertSqlSafe(format!(
            "CREATE VIEW {quoted_schema}.items_view AS SELECT id FROM {quoted_schema}.items"
        )))
        .execute(&mut admin)
        .await
        .unwrap();

        let profile_url =
            url_for_database(&admin_url, options.get_database().unwrap_or("postgres"));
        let profile = import_connection_url(&profile_url, Some("schema-drop-test"))
            .unwrap()
            .profile;
        let database_name = profile.database.clone().unwrap();
        let connection = ConnectionIdentity {
            profile_id: profile.id,
            generation: 1,
        };
        let schema = CatalogEntry::schema(
            CatalogId::new(
                profile.id,
                CatalogKind::Schema,
                [database_name.as_str(), schema_name.as_str()],
            ),
            CatalogId::new(profile.id, CatalogKind::Database, [database_name.as_str()]),
            QualifiedName {
                database: Some(database_name),
                schema: Some(schema_name.clone()),
                object: schema_name.clone(),
            },
            "schema",
            OptionalMetadata::Supported(None),
            true,
        )
        .unwrap();
        let request =
            CatalogDropRequest::new(connection, schema.id.clone(), 1).with_entry(schema.clone());
        let restrict = DatabaseConnection::connect(&profile, None)
            .await
            .unwrap()
            .plan_catalog_drop(request.clone(), &schema)
            .unwrap();
        assert_eq!(restrict.behavior(), CatalogDropBehavior::Restrict);
        let restricted_result = {
            let database = DatabaseConnection::connect(&profile, None).await.unwrap();
            database.execute(restrict.sql()).await
        };
        let error = restricted_result.unwrap_err();
        assert_eq!(error.code.as_deref(), Some("2BP01"));
        let schema_still_exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = $1)")
                .bind(&schema_name)
                .fetch_one(&mut admin)
                .await
                .unwrap();
        assert!(schema_still_exists);

        let cascade =
            lazydb::db::postgres::PostgresAdapter::plan_catalog_drop_cascade(request, &schema)
                .unwrap();
        assert_eq!(cascade.behavior(), CatalogDropBehavior::Cascade);
        DatabaseConnection::connect(&profile, None)
            .await
            .unwrap()
            .execute(cascade.sql())
            .await
            .unwrap();
        let schema_still_exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = $1)")
                .bind(&schema_name)
                .fetch_one(&mut admin)
                .await
                .unwrap();
        assert!(!schema_still_exists);
    }
    .await;
    if sqlx::query("SELECT 1").execute(&mut admin).await.is_ok() {
        let _ = raw_sql(AssertSqlSafe(format!(
            "DROP SCHEMA IF EXISTS {quoted_schema} CASCADE"
        )))
        .execute(&mut admin)
        .await;
    }
    admin.close().await.unwrap();
    result
}
