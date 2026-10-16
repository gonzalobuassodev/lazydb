use lazydb::{
    db::{
        catalog::{CatalogEntry, CatalogId, CatalogKind, OptionalMetadata, QualifiedName},
        catalog_drop::{CatalogDropExecutionTarget, CatalogDropRequest},
        clickhouse::ClickHouseAdapter,
    },
    identity::ConnectionIdentity,
};
use uuid::Uuid;

#[test]
fn database_and_table_drop_plans_quote_names_and_bind_maintenance_target() {
    let profile_id = Uuid::new_v4();
    let connection = ConnectionIdentity {
        profile_id,
        generation: 2,
    };
    let database_id = CatalogId::new(profile_id, CatalogKind::Database, ["analytics`blue"]);
    let database = CatalogEntry::database(
        database_id.clone(),
        QualifiedName {
            database: Some("analytics`blue".into()),
            schema: None,
            object: "analytics`blue".into(),
        },
        "DATABASE",
        OptionalMetadata::Unsupported,
        true,
    )
    .unwrap();
    let request = CatalogDropRequest::new(connection, database_id.clone(), 1)
        .with_entry(database.clone())
        .with_maintenance_database("default");
    let plan = ClickHouseAdapter::plan_catalog_drop(request, &database).unwrap();
    assert_eq!(plan.sql(), "DROP DATABASE `analytics``blue`");
    assert_eq!(
        plan.execution_target,
        CatalogDropExecutionTarget::MaintenanceDatabase("default".into())
    );

    let self_target = CatalogDropRequest::new(connection, database_id, 2)
        .with_entry(database.clone())
        .with_maintenance_database("analytics`blue");
    assert!(ClickHouseAdapter::plan_catalog_drop(self_target, &database).is_err());

    let schema = CatalogId::new(profile_id, CatalogKind::Schema, ["analytics", "analytics"]);
    let table_id = CatalogId::new(
        profile_id,
        CatalogKind::Table,
        ["analytics", "analytics", "events`raw"],
    );
    let table = CatalogEntry::relation(
        table_id.clone(),
        schema,
        QualifiedName {
            database: Some("analytics".into()),
            schema: Some("analytics".into()),
            object: "events`raw".into(),
        },
        "MergeTree",
        OptionalMetadata::Supported(None),
        true,
    )
    .unwrap();
    let request = CatalogDropRequest::new(connection, table_id, 3).with_entry(table.clone());
    let plan = ClickHouseAdapter::plan_catalog_drop(request, &table).unwrap();
    assert_eq!(plan.sql(), "DROP TABLE `analytics`.`events``raw`");
    assert_eq!(
        plan.execution_target,
        CatalogDropExecutionTarget::CurrentConnection
    );
}

#[test]
fn clickhouse_projection_dictionary_index_and_check_drop_fail_closed_with_specific_reasons() {
    use lazydb::db::{
        catalog::{CatalogMetadata, CatalogTarget, IndexMetadata},
        catalog_drop::CatalogDropError,
    };

    let profile_id = Uuid::new_v4();
    let connection = ConnectionIdentity {
        profile_id,
        generation: 4,
    };
    let database = "analytics";
    let table = CatalogId::new(
        profile_id,
        CatalogKind::Table,
        [database, database, "events"],
    );
    let schema = CatalogId::new(profile_id, CatalogKind::Schema, [database, database]);
    let projection_id = CatalogId::new(
        profile_id,
        CatalogKind::Projection,
        [database, database, "events", "by_id"],
    );
    let projection = CatalogEntry::projection(
        projection_id,
        table.clone(),
        QualifiedName {
            database: Some(database.into()),
            schema: Some(database.into()),
            object: "by_id".into(),
        },
        "Projection",
        IndexMetadata {
            columns: vec!["id".into()],
            unique: false,
        },
    )
    .unwrap();
    assert_eq!(projection.parent_id, Some(table.clone()));
    let target = CatalogTarget::projection_children(table.clone()).unwrap();
    assert_eq!(target.profile_id(), Some(profile_id));

    let dictionary_id = CatalogId::new(
        profile_id,
        CatalogKind::Dictionary,
        [database, database, "remote_users"],
    );
    let dictionary = CatalogEntry::object(
        dictionary_id,
        schema,
        QualifiedName {
            database: Some(database.into()),
            schema: Some(database.into()),
            object: "remote_users".into(),
        },
        "DICTIONARY",
        OptionalMetadata::Unsupported,
        false,
    )
    .unwrap();
    assert_eq!(dictionary.metadata, CatalogMetadata::None);

    for entry in [projection, dictionary] {
        let kind = entry.kind;
        let error = ClickHouseAdapter::plan_catalog_drop(
            CatalogDropRequest::new(connection, entry.id.clone(), 8).with_entry(entry.clone()),
            &entry,
        )
        .unwrap_err();
        let CatalogDropError::Unsupported {
            kind: actual,
            reason,
        } = error
        else {
            panic!("unsupported object should produce a fail-closed gate");
        };
        assert_eq!(actual, kind);
        assert!(reason.contains("ClickHouse"));
    }

    let dictionary_request = lazydb::db::catalog_mutation::CatalogObjectDefinitionRequest {
        connection,
        request_id: 9,
        catalog_epoch: 1,
        object: CatalogId::new(
            profile_id,
            CatalogKind::Dictionary,
            [database, database, "remote_users"],
        ),
        target: lazydb::model::execution_target::ExecutionTarget {
            profile_id,
            database: database.into(),
            schema: Some(database.into()),
        },
        principal: None,
    };
    assert!(dictionary_request.validate().is_err());
}
