mod support;

use lazydb::{
    db::{
        DatabaseConnection,
        catalog::{CatalogKind, CatalogRequest, CatalogRequestKey, CatalogTarget, ObjectGroup},
        catalog_mutation::{CatalogMutationAnchor, CatalogMutationMode, CatalogObjectType},
    },
    identity::ConnectionIdentity,
    model::catalog_editor::{CatalogDraft, TableDraft, ViewDraft},
    profile::import_connection_url,
};

#[tokio::test]
async fn mariadb_table_and_view_definition_round_trip_preserves_native_options() {
    let Some(url) = support::mariadb_test_url() else {
        return;
    };
    let imported = import_connection_url(&url, Some("MariaDB catalog mutation test")).unwrap();
    let database =
        DatabaseConnection::connect(&imported.profile, imported.transient_password.as_ref())
            .await
            .unwrap();
    let table = "lazydb_mariadb_catalog_probe";
    let view = "lazydb_mariadb_catalog_view";
    database
        .execute(&format!(
            "DROP VIEW IF EXISTS `{view}`; DROP TABLE IF EXISTS `{table}`; \
             CREATE TABLE `{table}` (id INT NOT NULL AUTO_INCREMENT PRIMARY KEY, value VARCHAR(32) NULL, \
             generated VARCHAR(64) AS (CONCAT(value, '-generated')) STORED) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='mariadb catalog'; \
             CREATE VIEW `{view}` AS SELECT id, value FROM `{table}`"
        ))
        .await
        .unwrap();
    let table_ddl = database
        .execute(&format!("SHOW CREATE TABLE `{table}`"))
        .await
        .unwrap();
    let table_sql = table_ddl.result_sets.last().unwrap().rows[0][1].clipboard_text();
    assert!(table_sql.contains("AUTO_INCREMENT"));
    assert!(table_sql.to_ascii_lowercase().contains("utf8mb4"));
    assert!(table_sql.contains("generated"));
    let view_ddl = database
        .execute(&format!("SHOW CREATE VIEW `{view}`"))
        .await
        .unwrap();
    assert!(
        view_ddl.result_sets.last().unwrap().rows[0][1]
            .clipboard_text()
            .to_ascii_uppercase()
            .contains("SELECT")
    );
    database
        .execute(&format!("DROP VIEW `{view}`; DROP TABLE `{table}`"))
        .await
        .unwrap();
    database.close().await;
}

#[tokio::test]
async fn mariadb_explorer_mutation_plans_execute_and_refresh_catalog() {
    let Some(url) = support::mariadb_test_url() else {
        return;
    };
    let imported = import_connection_url(&url, Some("MariaDB Explorer mutation test")).unwrap();
    let profile_id = imported.profile.id;
    let database =
        DatabaseConnection::connect(&imported.profile, imported.transient_password.as_ref())
            .await
            .unwrap();
    let database_name = database.probe().await.unwrap().database;
    let identity = ConnectionIdentity {
        profile_id,
        generation: 1,
    };
    let schema = lazydb::db::catalog::CatalogId::new(
        profile_id,
        CatalogKind::Schema,
        [database_name.clone(), database_name.clone()],
    );
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let table_name = format!("lazydb_explorer_{suffix}");
    let view_name = format!("lazydb_explorer_view_{suffix}");

    let mut table =
        TableDraft::new_for_database(&database_name, lazydb::profile::DatabaseKind::MariaDb);
    table.name.set(&table_name);
    table.columns[0].name.set("id");
    table.columns[0].native_type.set("INT");
    let table_request = lazydb::db::catalog_mutation::CatalogMutationRequest::new(
        identity,
        1,
        1,
        CatalogMutationMode::Create,
        CatalogMutationAnchor::Catalog(lazydb::db::catalog::CatalogId::new(
            profile_id,
            CatalogKind::Database,
            [database_name.clone()],
        )),
        CatalogObjectType::Catalog(CatalogKind::Table),
    )
    .unwrap()
    .with_current_database(database_name.clone());
    let table_plan = database
        .plan_catalog_mutation(table_request, CatalogDraft::Table(table), None)
        .unwrap();
    database
        .execute_catalog_mutation(&table_plan)
        .await
        .unwrap();

    let mut view = ViewDraft {
        name: Default::default(),
        schema: Default::default(),
        owner: Default::default(),
        comment: Default::default(),
        query: Default::default(),
        output_columns: Default::default(),
        security_barrier: lazydb::db::catalog_mutation::ViewOption::unavailable("unsupported"),
        security_invoker: lazydb::db::catalog_mutation::ViewOption::unavailable("unsupported"),
        check_option: lazydb::db::catalog_mutation::ViewOption::unavailable("unsupported"),
        focus: lazydb::model::catalog_editor::CatalogFormFocus::Name,
        clickhouse_native: None,
    };
    view.name.set(&view_name);
    view.schema.set(&database_name);
    view.query.set(format!("SELECT id FROM `{table_name}`"));
    let view_request = lazydb::db::catalog_mutation::CatalogMutationRequest::new(
        identity,
        2,
        1,
        CatalogMutationMode::Create,
        CatalogMutationAnchor::Group {
            schema: schema.clone(),
            group: ObjectGroup::Views,
        },
        CatalogObjectType::Catalog(CatalogKind::View),
    )
    .unwrap()
    .with_current_database(database_name.clone());
    let view_plan = database
        .plan_catalog_mutation(view_request, CatalogDraft::View(view), None)
        .unwrap();
    database.execute_catalog_mutation(&view_plan).await.unwrap();

    let view_refresh = CatalogTarget::objects(schema.clone(), ObjectGroup::Views).unwrap();
    let view_request = CatalogRequest {
        key: CatalogRequestKey {
            connection: identity,
            catalog_epoch: 1,
            request_id: 3,
            target: view_refresh,
            cursor: None,
        },
        scope: imported.profile.catalog_scope.clone(),
        page_size: 100,
    };
    let view_page = database.load_catalog_page(&view_request).await.unwrap();
    assert!(view_page.entries.iter().any(|entry| {
        entry.kind == CatalogKind::View && entry.id.native_path.last() == Some(&view_name)
    }));
    let table_request = CatalogRequest {
        key: CatalogRequestKey {
            connection: identity,
            catalog_epoch: 1,
            request_id: 4,
            target: CatalogTarget::objects(schema, ObjectGroup::Tables).unwrap(),
            cursor: None,
        },
        scope: imported.profile.catalog_scope.clone(),
        page_size: 100,
    };
    let table_page = database.load_catalog_page(&table_request).await.unwrap();
    assert!(table_page.entries.iter().any(|entry| {
        entry.kind == CatalogKind::Table && entry.id.native_path.last() == Some(&table_name)
    }));

    database
        .execute(&format!(
            "DROP VIEW `{view_name}`; DROP TABLE `{table_name}`"
        ))
        .await
        .unwrap();
    database.close().await;
}
