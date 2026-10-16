use lazydb::{
    db::{
        catalog::{CatalogId, CatalogKind, ObjectGroup},
        catalog_mutation::{
            CatalogMutationAnchor, CatalogMutationAvailability, CatalogMutationExecutionMode,
            CatalogMutationMode, CatalogMutationPlan, CatalogMutationRequest,
            CatalogMutationTarget, CatalogObjectDefinition, CatalogObjectType,
            CatalogSelectionHint, ClickHouseColumnDefinition, ClickHouseTableDefinition,
        },
        clickhouse::ClickHouseAdapter,
    },
    identity::ConnectionIdentity,
    model::catalog_editor::{CatalogDraft, DatabaseDraft, TableDraft},
    profile::{DatabaseKind, import_connection_url},
};
use uuid::Uuid;

fn clickhouse_test_url() -> Option<String> {
    match std::env::var("LAZYDB_TEST_CLICKHOUSE_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        Ok(_) => panic!("LAZYDB_TEST_CLICKHOUSE_URL must not be empty"),
        Err(std::env::VarError::NotPresent)
            if std::env::var("LAZYDB_REQUIRE_DATABASE_TESTS").as_deref() == Ok("1") =>
        {
            panic!("LAZYDB_TEST_CLICKHOUSE_URL is required for ClickHouse integration tests")
        }
        Err(std::env::VarError::NotPresent) => {
            eprintln!("SKIP: LAZYDB_TEST_CLICKHOUSE_URL is not set");
            None
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            panic!("LAZYDB_TEST_CLICKHOUSE_URL is not valid Unicode")
        }
    }
}

fn identity(profile_id: Uuid) -> ConnectionIdentity {
    ConnectionIdentity {
        profile_id,
        generation: 1,
    }
}

#[test]
fn database_create_binds_profile_and_uses_captured_maintenance_database() {
    let profile_id = Uuid::new_v4();
    let request = CatalogMutationRequest::new(
        identity(profile_id),
        1,
        4,
        CatalogMutationMode::Create,
        CatalogMutationAnchor::Profile { profile_id },
        CatalogObjectType::Catalog(CatalogKind::Database),
    )
    .unwrap()
    .with_current_database("default");
    let mut draft = DatabaseDraft::new_for_database(DatabaseKind::ClickHouse);
    draft.name = "analytics`blue".into();
    let plan = ClickHouseAdapter::plan_catalog_mutation(
        request.clone(),
        CatalogDraft::Database(draft),
        None,
        "default",
    )
    .unwrap();
    assert_eq!(plan.execution_target.database(), "default");
    assert_eq!(
        plan.statements(),
        &["CREATE DATABASE `analytics``blue`".to_owned()]
    );
    assert_eq!(
        plan.refresh,
        [lazydb::db::catalog::CatalogTarget::Databases]
    );

    let mut stale = request;
    stale.current_database = Some("other".into());
    assert!(
        ClickHouseAdapter::plan_catalog_mutation(
            stale,
            CatalogDraft::Database({
                let mut draft = DatabaseDraft::new_for_database(DatabaseKind::ClickHouse);
                draft.name = "analytics".into();
                draft
            }),
            None,
            "default",
        )
        .is_err()
    );
}

#[test]
fn table_create_rejects_wrong_group_or_database_and_only_opens_supported_create() {
    let profile_id = Uuid::new_v4();
    let schema = CatalogId::new(profile_id, CatalogKind::Schema, ["analytics", "analytics"]);
    let request = CatalogMutationRequest::new(
        identity(profile_id),
        2,
        8,
        CatalogMutationMode::Create,
        CatalogMutationAnchor::Group {
            schema: schema.clone(),
            group: ObjectGroup::Tables,
        },
        CatalogObjectType::Catalog(CatalogKind::Table),
    )
    .unwrap()
    .with_current_database("default");
    let mut draft = TableDraft::new_for_database("analytics", DatabaseKind::ClickHouse);
    draft.name = "events".into();
    draft.columns[0].name = "id".into();
    draft.columns[0].native_type = "UInt64".into();
    let plan = ClickHouseAdapter::plan_catalog_mutation(
        request,
        CatalogDraft::Table(draft),
        None,
        "default",
    )
    .unwrap();
    assert_eq!(plan.execution_target.database(), "analytics");
    assert!(plan.sql().contains("ENGINE = MergeTree ORDER BY tuple()"));

    let capabilities = ClickHouseAdapter::catalog_mutation_capabilities();
    assert_eq!(
        capabilities.create_availability(CatalogObjectType::Catalog(CatalogKind::Table)),
        Some(CatalogMutationAvailability::Available)
    );
    assert_eq!(
        capabilities.create_availability(CatalogObjectType::Catalog(CatalogKind::View)),
        Some(CatalogMutationAvailability::Available)
    );
    assert_eq!(
        capabilities.edit_availability(CatalogObjectType::Catalog(CatalogKind::Table)),
        Some(CatalogMutationAvailability::Available)
    );
    assert_eq!(
        capabilities.create_availability(CatalogObjectType::Catalog(CatalogKind::MaterializedView)),
        Some(CatalogMutationAvailability::Unavailable {
            reason: "ClickHouse materialized view creation requires explicit target and engine controls"
        })
    );
    assert_eq!(
        capabilities.edit_availability(CatalogObjectType::Catalog(CatalogKind::MaterializedView)),
        Some(CatalogMutationAvailability::Unavailable {
            reason: "ClickHouse materialized view edits require native target and engine controls"
        })
    );
    assert_eq!(
        capabilities.create_availability(CatalogObjectType::Catalog(CatalogKind::Index)),
        Some(CatalogMutationAvailability::Unavailable {
            reason: "ClickHouse skipping indexes require a native table baseline and explicit TYPE/GRANULARITY controls"
        })
    );
    assert_eq!(
        capabilities.edit_availability(CatalogObjectType::Catalog(CatalogKind::Index)),
        Some(CatalogMutationAvailability::Unavailable {
            reason: "ClickHouse skipping-index definition loading and ALTER planning are not implemented"
        })
    );
    assert_eq!(
        capabilities.create_availability(CatalogObjectType::Catalog(CatalogKind::CheckConstraint)),
        Some(CatalogMutationAvailability::Unavailable {
            reason: "ClickHouse check constraints require a native table baseline and server-version-aware ALTER controls"
        })
    );
    assert_eq!(
        capabilities.edit_availability(CatalogObjectType::Catalog(CatalogKind::CheckConstraint)),
        Some(CatalogMutationAvailability::Unavailable {
            reason: "ClickHouse check-constraint definition loading and ALTER planning are not implemented"
        })
    );

    let imported =
        import_connection_url("clickhouse://localhost/analytics", Some("scope")).unwrap();
    assert_eq!(imported.profile.database.as_deref(), Some("analytics"));
}

fn native_table_definition(profile_id: Uuid) -> (CatalogId, ClickHouseTableDefinition) {
    let object = CatalogId::new(
        profile_id,
        CatalogKind::Table,
        ["analytics", "analytics", "events"],
    );
    let definition = ClickHouseTableDefinition {
        database: "analytics".into(),
        name: "events".into(),
        engine: "ReplacingMergeTree".into(),
        columns: vec![ClickHouseColumnDefinition {
            name: "id".into(),
            ordinal_position: 1,
            native_type: "Nullable(UInt64)".into(),
            default_kind: "MATERIALIZED".into(),
            default_expression: "toUInt64(1)".into(),
            comment: lazydb::db::catalog::OptionalMetadata::Supported(None),
        }],
        canonical_ddl: "CREATE TABLE analytics.events (id Nullable(UInt64) MATERIALIZED toUInt64(1)) ENGINE = ReplacingMergeTree ORDER BY id TTL id + INTERVAL 1 DAY SETTINGS index_granularity = 8192".into(),
        baseline_fingerprint: "sha256:baseline-v1".into(),
        comment: lazydb::db::catalog::OptionalMetadata::Supported(Some("events".into())),
    };
    (object, definition)
}

#[test]
fn clickhouse_native_table_edit_preserves_opaque_ddl_and_emits_incremental_rename() {
    let profile_id = Uuid::new_v4();
    let (object, definition) = native_table_definition(profile_id);
    let request = CatalogMutationRequest::new(
        identity(profile_id),
        30,
        9,
        CatalogMutationMode::Edit,
        CatalogMutationAnchor::Catalog(object),
        CatalogObjectType::Catalog(CatalogKind::Table),
    )
    .unwrap()
    .with_current_database("default");
    let mut draft = TableDraft::from_clickhouse_definition(&definition);
    draft.name = "events_v2".into();
    let original_ddl = definition.canonical_ddl.clone();
    let plan = ClickHouseAdapter::plan_catalog_mutation(
        request,
        CatalogDraft::Table(draft),
        Some(CatalogObjectDefinition::ClickHouseTable(definition)),
        "default",
    )
    .unwrap();
    assert_eq!(
        plan.statements(),
        &["RENAME TABLE `analytics`.`events` TO `analytics`.`events_v2`".to_owned()]
    );
    assert!(!plan.sql().contains("CREATE TABLE"));
    assert!(original_ddl.contains("ReplacingMergeTree"));
    assert!(original_ddl.contains("TTL id + INTERVAL 1 DAY"));
    assert!(original_ddl.contains("SETTINGS index_granularity = 8192"));
    assert_eq!(
        plan.baseline_fingerprint.as_deref(),
        Some("sha256:baseline-v1")
    );
}

#[test]
fn clickhouse_native_table_edit_plans_comment_and_appended_column_as_incremental_alters() {
    let profile_id = Uuid::new_v4();
    let (object, definition) = native_table_definition(profile_id);
    let request = || {
        CatalogMutationRequest::new(
            identity(profile_id),
            32,
            9,
            CatalogMutationMode::Edit,
            CatalogMutationAnchor::Catalog(object.clone()),
            CatalogObjectType::Catalog(CatalogKind::Table),
        )
        .unwrap()
        .with_current_database("default")
    };
    let mut comment_draft = TableDraft::from_clickhouse_definition(&definition);
    comment_draft.comment = "retained rows".into();
    let comment_plan = ClickHouseAdapter::plan_catalog_mutation(
        request(),
        CatalogDraft::Table(comment_draft),
        Some(CatalogObjectDefinition::ClickHouseTable(definition.clone())),
        "default",
    )
    .unwrap();
    assert_eq!(
        comment_plan.statements(),
        &["ALTER TABLE `analytics`.`events` MODIFY COMMENT 'retained rows'".to_owned()]
    );
    assert!(!comment_plan.sql().contains("CREATE TABLE"));

    let mut escaped_comment = TableDraft::from_clickhouse_definition(&definition);
    escaped_comment.comment = "owner's \\ note".into();
    let escaped_plan = ClickHouseAdapter::plan_catalog_mutation(
        request(),
        CatalogDraft::Table(escaped_comment),
        Some(CatalogObjectDefinition::ClickHouseTable(definition.clone())),
        "default",
    )
    .unwrap();
    assert!(escaped_plan.sql().contains("owner\\'s \\\\ note"));

    let mut column_draft = TableDraft::from_clickhouse_definition(&definition);
    column_draft.columns.push(
        lazydb::model::catalog_editor::ColumnDraft::new_added_for_database(
            DatabaseKind::ClickHouse,
        ),
    );
    column_draft.columns[1].name = "region".into();
    column_draft.columns[1].native_type = "LowCardinality(String)".into();
    let column_plan = ClickHouseAdapter::plan_catalog_mutation(
        request(),
        CatalogDraft::Table(column_draft),
        Some(CatalogObjectDefinition::ClickHouseTable(definition)),
        "default",
    )
    .unwrap();
    assert_eq!(
        column_plan.statements(),
        &[
            "ALTER TABLE `analytics`.`events` ADD COLUMN `region` LowCardinality(String)"
                .to_owned()
        ]
    );
    assert!(!column_plan.sql().contains("CREATE TABLE"));
}

#[test]
fn clickhouse_native_table_edit_rejects_stale_baseline_and_unknown_shapes() {
    let profile_id = Uuid::new_v4();
    let (object, definition) = native_table_definition(profile_id);
    let request = || {
        CatalogMutationRequest::new(
            identity(profile_id),
            31,
            9,
            CatalogMutationMode::Edit,
            CatalogMutationAnchor::Catalog(object.clone()),
            CatalogObjectType::Catalog(CatalogKind::Table),
        )
        .unwrap()
        .with_current_database("default")
    };
    let stale = request();
    let mut draft = TableDraft::from_clickhouse_definition(&definition);
    draft.name = "events_v2".into();
    draft.clickhouse_baseline_fingerprint = Some("sha256:stale".into());
    assert!(
        ClickHouseAdapter::plan_catalog_mutation(
            stale,
            CatalogDraft::Table(draft),
            Some(CatalogObjectDefinition::ClickHouseTable(definition.clone())),
            "default"
        )
        .is_err()
    );

    let refreshed_definition = ClickHouseTableDefinition {
        baseline_fingerprint: "sha256:changed-on-server".into(),
        ..definition.clone()
    };
    let mut draft = TableDraft::from_clickhouse_definition(&definition);
    draft.name = "events_v2".into();
    assert!(
        ClickHouseAdapter::plan_catalog_mutation(
            request(),
            CatalogDraft::Table(draft),
            Some(CatalogObjectDefinition::ClickHouseTable(
                refreshed_definition
            )),
            "default",
        )
        .is_err()
    );

    let mut draft = TableDraft::from_clickhouse_definition(&definition);
    draft.name = "events_v2".into();
    draft.clickhouse_engine = "Memory".into();
    assert!(
        ClickHouseAdapter::plan_catalog_mutation(
            request(),
            CatalogDraft::Table(draft),
            Some(CatalogObjectDefinition::ClickHouseTable(definition)),
            "default"
        )
        .is_err()
    );
}

#[test]
fn clickhouse_view_create_and_edit_use_native_incremental_ddl() {
    use lazydb::model::catalog_editor::{CatalogFormFocus, ViewDraft};
    let profile_id = Uuid::new_v4();
    let schema = CatalogId::new(profile_id, CatalogKind::Schema, ["analytics", "analytics"]);
    let request = || {
        CatalogMutationRequest::new(
            identity(profile_id),
            80,
            9,
            CatalogMutationMode::Create,
            CatalogMutationAnchor::Catalog(schema.clone()),
            CatalogObjectType::Catalog(CatalogKind::View),
        )
        .unwrap()
        .with_current_database("default")
    };
    let mut draft = ViewDraft {
        name: "events_view".into(),
        schema: "analytics".into(),
        owner: Default::default(),
        comment: Default::default(),
        query: "SELECT id FROM analytics.events".into(),
        output_columns: Default::default(),
        security_barrier: lazydb::db::catalog_mutation::ViewOption::unavailable("n/a"),
        security_invoker: lazydb::db::catalog_mutation::ViewOption::unavailable("n/a"),
        check_option: lazydb::db::catalog_mutation::ViewOption::unavailable("n/a"),
        focus: CatalogFormFocus::Name,
        clickhouse_native: None,
    };
    let create = ClickHouseAdapter::plan_catalog_mutation(
        request(),
        CatalogDraft::View(draft.clone()),
        None,
        "default",
    )
    .unwrap();
    assert_eq!(
        create.statements(),
        &["CREATE VIEW `analytics`.`events_view` AS SELECT id FROM analytics.events".to_owned()]
    );

    let native = lazydb::db::catalog_mutation::ClickHouseViewDefinition {
        database: "analytics".into(),
        name: "events_view".into(),
        canonical_ddl:
            "CREATE VIEW analytics.events_view\n( id UInt64 )\nAS SELECT id FROM analytics.events"
                .into(),
        query: "SELECT id FROM analytics.events".into(),
        comment: lazydb::db::catalog::OptionalMetadata::Supported(None),
        baseline_fingerprint: "sha256:view".into(),
    };
    let baseline = lazydb::db::catalog_mutation::CatalogObjectDefinition::View(
        lazydb::db::catalog_mutation::ViewDefinition {
            database: "analytics".into(),
            schema: "analytics".into(),
            name: "events_view".into(),
            owner: String::new(),
            comment: native.comment.clone(),
            query: native.query.clone(),
            output_columns: vec![],
            security_barrier: lazydb::db::catalog_mutation::ViewOption::unavailable("n/a"),
            security_invoker: lazydb::db::catalog_mutation::ViewOption::unavailable("n/a"),
            check_option: lazydb::db::catalog_mutation::ViewOption::unavailable("n/a"),
            baseline_fingerprint: native.baseline_fingerprint.clone(),
            clickhouse_native: Some(native.clone()),
        },
    );
    draft.name = "events_view_v2".into();
    draft.clickhouse_native = Some(native);
    let edit_request = CatalogMutationRequest::new(
        identity(profile_id),
        81,
        9,
        CatalogMutationMode::Edit,
        CatalogMutationAnchor::Catalog(CatalogId::new(
            profile_id,
            CatalogKind::View,
            ["analytics", "analytics", "events_view"],
        )),
        CatalogObjectType::Catalog(CatalogKind::View),
    )
    .unwrap()
    .with_current_database("default");
    let edit = ClickHouseAdapter::plan_catalog_mutation(
        edit_request,
        CatalogDraft::View(draft),
        Some(baseline),
        "default",
    )
    .unwrap();
    assert_eq!(
        edit.statements(),
        &["RENAME TABLE `analytics`.`events_view` TO `analytics`.`events_view_v2`".to_owned()]
    );
}

#[test]
fn clickhouse_view_ddl_parser_preserves_query_and_materialized_target_separately() {
    let view = lazydb::db::clickhouse::definition::extract_view_query_and_target(
        "CREATE VIEW analytics.v (id UInt64) AS SELECT id FROM t",
        CatalogKind::View,
    )
    .unwrap();
    assert_eq!(view.0, "SELECT id FROM t");
    assert_eq!(view.1, None);
    let mv = lazydb::db::clickhouse::definition::extract_view_query_and_target(
        "CREATE MATERIALIZED VIEW analytics.mv TO `analytics`.`dest` ENGINE = MergeTree ORDER BY id AS SELECT id FROM t",
        CatalogKind::MaterializedView,
    ).unwrap();
    assert_eq!(mv.0, "SELECT id FROM t");
    assert_eq!(mv.1.as_deref(), Some("analytics.dest"));
    let populated = lazydb::db::clickhouse::definition::extract_view_query_and_target(
        "CREATE MATERIALIZED VIEW analytics.mv ENGINE = MergeTree ORDER BY id POPULATE AS SELECT id FROM t",
        CatalogKind::MaterializedView,
    )
    .unwrap();
    assert_eq!(populated.0, "SELECT id FROM t");
    assert_eq!(populated.1, None);
    let self_stored = lazydb::db::clickhouse::definition::extract_view_query_and_target(
        "CREATE MATERIALIZED VIEW analytics.mv ENGINE = MergeTree ORDER BY id AS SELECT id FROM t",
        CatalogKind::MaterializedView,
    )
    .unwrap();
    assert_eq!(self_stored.0, "SELECT id FROM t");
    assert_eq!(self_stored.1, None);
}

#[test]
fn clickhouse_view_native_draft_keeps_full_ddl_baseline_and_query_read_only() {
    let native = lazydb::db::catalog_mutation::ClickHouseViewDefinition {
        database: "analytics".into(),
        name: "events_view".into(),
        canonical_ddl: "CREATE VIEW analytics.events_view (id UInt64) AS SELECT id FROM events"
            .into(),
        query: "SELECT id FROM events".into(),
        comment: lazydb::db::catalog::OptionalMetadata::Supported(None),
        baseline_fingerprint: "sha256:view".into(),
    };
    let draft = lazydb::model::catalog_editor::ViewDraft::from_clickhouse_native(&native);
    assert_eq!(draft.clickhouse_native.as_ref(), Some(&native));
    assert_eq!(draft.query.value(), native.query);
    assert!(!draft.security_barrier.availability.is_available());
}

#[test]
fn clickhouse_view_definition_request_is_bound_to_owning_database() {
    let profile_id = Uuid::new_v4();
    let object = CatalogId::new(
        profile_id,
        CatalogKind::View,
        ["analytics", "analytics", "events_view"],
    );
    let request = |database: &str| lazydb::db::catalog_mutation::CatalogObjectDefinitionRequest {
        connection: identity(profile_id),
        request_id: 91,
        catalog_epoch: 4,
        object: object.clone(),
        target: lazydb::model::execution_target::ExecutionTarget {
            profile_id,
            database: database.into(),
            schema: Some(database.into()),
        },
        principal: None,
    };
    assert!(request("analytics").validate().is_ok());
    assert!(request("default").validate().is_err());
}

#[test]
fn clickhouse_edit_plan_stale_fingerprint_is_checked_against_reloaded_definition() {
    let profile_id = Uuid::new_v4();
    let (object, definition) = native_table_definition(profile_id);
    let request = CatalogMutationRequest::new(
        identity(profile_id),
        33,
        9,
        CatalogMutationMode::Edit,
        CatalogMutationAnchor::Catalog(object.clone()),
        CatalogObjectType::Catalog(CatalogKind::Table),
    )
    .unwrap()
    .with_current_database("default");
    let plan = CatalogMutationPlan::new(
        request,
        CatalogObjectType::Catalog(CatalogKind::Table),
        CatalogMutationExecutionMode::Autocommit,
        CatalogMutationTarget::maintenance("analytics").unwrap(),
        vec![
            lazydb::db::catalog::CatalogTarget::objects(
                CatalogId::new(profile_id, CatalogKind::Schema, ["analytics", "analytics"]),
                ObjectGroup::Tables,
            )
            .unwrap(),
        ],
        CatalogSelectionHint::Object(object.clone()),
        Some(definition.baseline_fingerprint.clone()),
        Vec::new(),
        vec!["RENAME TABLE `analytics`.`events` TO `analytics`.`events_v2`".into()],
    )
    .unwrap();
    let reloaded = ClickHouseTableDefinition {
        baseline_fingerprint: "sha256:changed-on-server".into(),
        ..definition
    };
    assert!(!plan.baseline_matches_definition(&CatalogObjectDefinition::ClickHouseTable(reloaded)));
    assert!(
        plan.baseline_matches_definition(&CatalogObjectDefinition::ClickHouseTable(
            native_table_definition(profile_id).1
        ))
    );
}

#[tokio::test]
async fn clickhouse_live_create_load_ddl_and_drop_uses_isolated_database() {
    let Some(url) = clickhouse_test_url() else {
        return;
    };
    let mut imported = import_connection_url(&url, Some("clickhouse-mutation-test")).unwrap();
    let database = format!("lazydb_test_{}", Uuid::new_v4().simple());
    let profile_id = imported.profile.id;
    if let lazydb::profile::CatalogSelection::Selected(databases) =
        &mut imported.profile.catalog_scope.databases
    {
        databases.push(lazydb::profile::DatabaseScope {
            name: database.clone(),
            schemas: lazydb::profile::CatalogSelection::All,
        });
    }
    let base = lazydb::db::DatabaseConnection::connect(
        &imported.profile,
        imported.transient_password.as_ref(),
    )
    .await
    .expect("connect with isolated database included in captured scope");
    let maintenance_database = imported
        .profile
        .database
        .clone()
        .unwrap_or_else(|| "default".into());
    let database_request = CatalogMutationRequest::new(
        identity(profile_id),
        9,
        1,
        CatalogMutationMode::Create,
        CatalogMutationAnchor::Profile { profile_id },
        CatalogObjectType::Catalog(CatalogKind::Database),
    )
    .unwrap()
    .with_current_database(maintenance_database.clone());
    let mut database_draft = DatabaseDraft::new_for_database(DatabaseKind::ClickHouse);
    database_draft.name = database.clone().into();
    let database_plan = base
        .plan_catalog_mutation(
            database_request,
            CatalogDraft::Database(database_draft),
            None,
        )
        .unwrap();
    base.execute_catalog_mutation(&database_plan)
        .await
        .expect("create isolated database through catalog mutation planner");
    let schema = CatalogId::new(profile_id, CatalogKind::Schema, [&database, &database]);
    let request = CatalogMutationRequest::new(
        identity(profile_id),
        10,
        1,
        CatalogMutationMode::Create,
        CatalogMutationAnchor::Group {
            schema: schema.clone(),
            group: ObjectGroup::Tables,
        },
        CatalogObjectType::Catalog(CatalogKind::Table),
    )
    .unwrap()
    .with_current_database(base.probe().await.unwrap().database);
    let mut draft = TableDraft::new_for_database(&database, DatabaseKind::ClickHouse);
    draft.name = "events".into();
    draft.columns[0].name = "id".into();
    draft.columns[0].native_type = "UInt64".into();
    let plan = base
        .plan_catalog_mutation(request, CatalogDraft::Table(draft), None)
        .unwrap();
    base.execute_catalog_mutation(&plan)
        .await
        .expect("create ClickHouse table");

    let page = base
        .load_catalog_page(&lazydb::db::catalog::CatalogRequest {
            key: lazydb::db::catalog::CatalogRequestKey {
                connection: identity(profile_id),
                catalog_epoch: 1,
                request_id: 11,
                target: lazydb::db::catalog::CatalogTarget::objects(schema, ObjectGroup::Tables)
                    .unwrap(),
                cursor: None,
            },
            scope: imported.profile.catalog_scope.clone(),
            page_size: 20,
        })
        .await
        .expect("load created table from catalog");
    let entry = page
        .entries
        .into_iter()
        .find(|entry| entry.qualified_name.object == "events")
        .expect("created table catalog entry");
    let ddl = base
        .relation_ddl_with_scope(&entry.id, &imported.profile.catalog_scope)
        .await
        .expect("load canonical ClickHouse DDL");
    assert!(ddl.sql.contains("MergeTree"));
    assert!(ddl.sql.contains("ORDER BY tuple()"));
    let definition = base
        .load_catalog_object_definition(
            &lazydb::db::catalog_mutation::CatalogObjectDefinitionRequest {
                connection: identity(profile_id),
                request_id: 14,
                catalog_epoch: 1,
                object: entry.id.clone(),
                target: lazydb::model::execution_target::ExecutionTarget {
                    profile_id,
                    database: database.clone(),
                    schema: Some(database.clone()),
                },
                principal: None,
            },
        )
        .await
        .expect("load native table definition");
    assert!(matches!(
        definition,
        lazydb::db::catalog_mutation::CatalogObjectDefinition::ClickHouseTable(_)
    ));

    base.execute(&format!("INSERT INTO `{database}`.`events` VALUES (42)"))
        .await
        .expect("insert data before native incremental edits");
    let definition = base
        .load_catalog_object_definition(
            &lazydb::db::catalog_mutation::CatalogObjectDefinitionRequest {
                connection: identity(profile_id),
                request_id: 15,
                catalog_epoch: 1,
                object: entry.id.clone(),
                target: lazydb::model::execution_target::ExecutionTarget {
                    profile_id,
                    database: database.clone(),
                    schema: Some(database.clone()),
                },
                principal: None,
            },
        )
        .await
        .expect("reload baseline before editing");
    let lazydb::db::catalog_mutation::CatalogObjectDefinition::ClickHouseTable(definition) =
        definition
    else {
        panic!("native ClickHouse table baseline expected");
    };
    let edit_request = |request_id, object: CatalogId| {
        CatalogMutationRequest::new(
            identity(profile_id),
            request_id,
            1,
            CatalogMutationMode::Edit,
            CatalogMutationAnchor::Catalog(object),
            CatalogObjectType::Catalog(CatalogKind::Table),
        )
        .unwrap()
        .with_current_database(maintenance_database.clone())
    };
    let mut rename_draft = TableDraft::from_clickhouse_definition(&definition);
    rename_draft.name = "events_renamed".into();
    let rename_plan = base
        .plan_catalog_mutation(
            edit_request(16, entry.id.clone()),
            CatalogDraft::Table(rename_draft),
            Some(
                lazydb::db::catalog_mutation::CatalogObjectDefinition::ClickHouseTable(definition),
            ),
        )
        .unwrap();
    base.execute_catalog_mutation(&rename_plan)
        .await
        .expect("rename table incrementally");

    let renamed_id = CatalogId::new(
        profile_id,
        CatalogKind::Table,
        [&database, &database, "events_renamed"],
    );
    let load_native = |request_id, object: CatalogId| {
        lazydb::db::catalog_mutation::CatalogObjectDefinitionRequest {
            connection: identity(profile_id),
            request_id,
            catalog_epoch: 1,
            object,
            target: lazydb::model::execution_target::ExecutionTarget {
                profile_id,
                database: database.clone(),
                schema: Some(database.clone()),
            },
            principal: None,
        }
    };
    let loaded = base
        .load_catalog_object_definition(&load_native(17, renamed_id.clone()))
        .await
        .expect("reload renamed table");
    let lazydb::db::catalog_mutation::CatalogObjectDefinition::ClickHouseTable(definition) = loaded
    else {
        panic!("native definition expected")
    };
    let mut comment_draft = TableDraft::from_clickhouse_definition(&definition);
    comment_draft.comment = "retained-data-check".into();
    let comment_plan = base
        .plan_catalog_mutation(
            edit_request(18, renamed_id.clone()),
            CatalogDraft::Table(comment_draft),
            Some(
                lazydb::db::catalog_mutation::CatalogObjectDefinition::ClickHouseTable(definition),
            ),
        )
        .unwrap();
    base.execute_catalog_mutation(&comment_plan)
        .await
        .expect("alter table comment incrementally");

    let loaded = base
        .load_catalog_object_definition(&load_native(19, renamed_id.clone()))
        .await
        .expect("reload commented table");
    let lazydb::db::catalog_mutation::CatalogObjectDefinition::ClickHouseTable(definition) = loaded
    else {
        panic!("native definition expected")
    };
    let mut add_draft = TableDraft::from_clickhouse_definition(&definition);
    add_draft.columns.push(
        lazydb::model::catalog_editor::ColumnDraft::new_added_for_database(
            DatabaseKind::ClickHouse,
        ),
    );
    add_draft.columns[1].name = "region".into();
    add_draft.columns[1].native_type = "String".into();
    let add_plan = base
        .plan_catalog_mutation(
            edit_request(20, renamed_id.clone()),
            CatalogDraft::Table(add_draft),
            Some(
                lazydb::db::catalog_mutation::CatalogObjectDefinition::ClickHouseTable(definition),
            ),
        )
        .unwrap();
    base.execute_catalog_mutation(&add_plan)
        .await
        .expect("add column incrementally");

    let loaded = base
        .load_catalog_object_definition(&load_native(21, renamed_id.clone()))
        .await
        .expect("reload edited native table");
    let lazydb::db::catalog_mutation::CatalogObjectDefinition::ClickHouseTable(definition) = loaded
    else {
        panic!("native definition expected")
    };
    assert_eq!(definition.name, "events_renamed");
    assert!(definition.canonical_ddl.contains("retained-data-check"));
    assert!(
        definition
            .columns
            .iter()
            .any(|column| column.name == "region")
    );
    let rows = base
        .execute(&format!(
            "SELECT count() FROM `{database}`.`events_renamed` WHERE id = 42"
        ))
        .await
        .expect("query row after rename/comment/add-column");
    assert_eq!(rows.result_sets[0].rows[0][0].display_text(16), "1");

    let renamed_entry = base
        .load_catalog_page(&lazydb::db::catalog::CatalogRequest {
            key: lazydb::db::catalog::CatalogRequestKey {
                connection: identity(profile_id),
                catalog_epoch: 1,
                request_id: 22,
                target: lazydb::db::catalog::CatalogTarget::objects(
                    CatalogId::new(profile_id, CatalogKind::Schema, [&database, &database]),
                    ObjectGroup::Tables,
                )
                .unwrap(),
                cursor: None,
            },
            scope: imported.profile.catalog_scope.clone(),
            page_size: 20,
        })
        .await
        .unwrap()
        .entries
        .into_iter()
        .find(|entry| entry.qualified_name.object == "events_renamed")
        .unwrap();
    let drop_request = lazydb::db::catalog_drop::CatalogDropRequest::new(
        identity(profile_id),
        renamed_entry.id.clone(),
        12,
    )
    .with_entry(renamed_entry.clone());
    let drop_plan = base
        .plan_catalog_drop(drop_request, &renamed_entry)
        .unwrap();
    base.execute(drop_plan.sql())
        .await
        .expect("drop created table");
    let database_id = CatalogId::new(profile_id, CatalogKind::Database, [&database]);
    let database_entry = lazydb::db::catalog::CatalogEntry::database(
        database_id.clone(),
        lazydb::db::catalog::QualifiedName {
            database: Some(database.clone()),
            schema: None,
            object: database.clone(),
        },
        "DATABASE",
        lazydb::db::catalog::OptionalMetadata::Unsupported,
        true,
    )
    .unwrap();
    let database_drop_request =
        lazydb::db::catalog_drop::CatalogDropRequest::new(identity(profile_id), database_id, 13)
            .with_entry(database_entry.clone())
            .with_maintenance_database(maintenance_database);
    let database_drop = base
        .plan_catalog_drop(database_drop_request, &database_entry)
        .unwrap();
    base.execute(database_drop.sql())
        .await
        .expect("drop isolated database through catalog drop planner");
    base.close().await;
}

#[tokio::test]
async fn clickhouse_live_view_create_load_rename_reload_and_drop_preserves_query() {
    let Some(url) = clickhouse_test_url() else {
        return;
    };
    use lazydb::model::catalog_editor::{CatalogFormFocus, ViewDraft};

    let imported = import_connection_url(&url, Some("clickhouse-view-mutation-test")).unwrap();
    let database = lazydb::db::DatabaseConnection::connect(
        &imported.profile,
        imported.transient_password.as_ref(),
    )
    .await
    .expect("connect to configured ClickHouse test server");
    let profile_id = imported.profile.id;
    let database_name = database.probe().await.unwrap().database;
    let suffix = Uuid::new_v4().simple().to_string();
    let view_name = format!("lazydb_view_{suffix}");
    let renamed_view = format!("{view_name}_renamed");
    let schema = CatalogId::new(
        profile_id,
        CatalogKind::Schema,
        [database_name.as_str(), database_name.as_str()],
    );
    let query = "SELECT toUInt64(7) AS id";
    let mut draft = ViewDraft {
        name: view_name.clone().into(),
        schema: database_name.clone().into(),
        owner: Default::default(),
        comment: Default::default(),
        query: query.into(),
        output_columns: Default::default(),
        security_barrier: lazydb::db::catalog_mutation::ViewOption::unavailable(
            "not applicable to ClickHouse",
        ),
        security_invoker: lazydb::db::catalog_mutation::ViewOption::unavailable(
            "not applicable to ClickHouse",
        ),
        check_option: lazydb::db::catalog_mutation::ViewOption::unavailable(
            "not applicable to ClickHouse",
        ),
        focus: CatalogFormFocus::Name,
        clickhouse_native: None,
    };
    let create_request = CatalogMutationRequest::new(
        identity(profile_id),
        100,
        1,
        CatalogMutationMode::Create,
        CatalogMutationAnchor::Catalog(schema),
        CatalogObjectType::Catalog(CatalogKind::View),
    )
    .unwrap()
    .with_current_database(database_name.clone());
    let create_plan = database
        .plan_catalog_mutation(create_request, CatalogDraft::View(draft.clone()), None)
        .unwrap();
    database
        .execute_catalog_mutation(&create_plan)
        .await
        .expect("create isolated ClickHouse view");

    let load = |request_id, name: &str| {
        let object = CatalogId::new(
            profile_id,
            CatalogKind::View,
            [database_name.as_str(), database_name.as_str(), name],
        );
        lazydb::db::catalog_mutation::CatalogObjectDefinitionRequest {
            connection: identity(profile_id),
            request_id,
            catalog_epoch: 1,
            object,
            target: lazydb::model::execution_target::ExecutionTarget {
                profile_id,
                database: database_name.clone(),
                schema: Some(database_name.clone()),
            },
            principal: None,
        }
    };
    let definition = database
        .load_catalog_object_definition(&load(101, &view_name))
        .await
        .expect("load native view definition");
    let CatalogObjectDefinition::View(definition) = definition else {
        panic!("native ClickHouse View definition expected");
    };
    let native = definition
        .clickhouse_native
        .as_ref()
        .expect("canonical ClickHouse DDL captured");
    assert_eq!(native.query, query);
    assert!(native.canonical_ddl.contains(query));

    draft = ViewDraft::from_clickhouse_native(native);
    draft.name = renamed_view.clone().into();
    let edit_request = CatalogMutationRequest::new(
        identity(profile_id),
        102,
        1,
        CatalogMutationMode::Edit,
        CatalogMutationAnchor::Catalog(load(102, &view_name).object),
        CatalogObjectType::Catalog(CatalogKind::View),
    )
    .unwrap()
    .with_current_database(database_name.clone());
    let edit_plan = database
        .plan_catalog_mutation(
            edit_request,
            CatalogDraft::View(draft),
            Some(CatalogObjectDefinition::View(definition)),
        )
        .unwrap();
    database
        .execute_catalog_mutation(&edit_plan)
        .await
        .expect("rename isolated ClickHouse view");

    let definition = database
        .load_catalog_object_definition(&load(103, &renamed_view))
        .await
        .expect("reload renamed native view");
    let CatalogObjectDefinition::View(definition) = definition else {
        panic!("native ClickHouse View definition expected after rename");
    };
    let native = definition.clickhouse_native.as_ref().unwrap();
    assert_eq!(native.name, renamed_view);
    assert_eq!(native.query, query);
    assert!(native.canonical_ddl.contains(query));

    let entry = lazydb::db::catalog::CatalogEntry::relation(
        load(104, &renamed_view).object,
        CatalogId::new(
            profile_id,
            CatalogKind::Schema,
            [database_name.as_str(), database_name.as_str()],
        ),
        lazydb::db::catalog::QualifiedName {
            database: Some(database_name.clone()),
            schema: Some(database_name.clone()),
            object: renamed_view,
        },
        "View",
        lazydb::db::catalog::OptionalMetadata::Supported(None),
        true,
    )
    .unwrap();
    let drop_request = lazydb::db::catalog_drop::CatalogDropRequest::new(
        identity(profile_id),
        entry.id.clone(),
        105,
    )
    .with_entry(entry.clone());
    let drop_plan = database.plan_catalog_drop(drop_request, &entry).unwrap();
    database
        .execute(drop_plan.sql())
        .await
        .expect("drop isolated ClickHouse view");
    database.close().await;
}

#[tokio::test]
async fn clickhouse_live_materialized_view_to_target_loads_and_drops_without_replay() {
    let Some(url) = clickhouse_test_url() else {
        return;
    };
    let imported = import_connection_url(&url, Some("clickhouse-mv-mutation-test")).unwrap();
    let database = lazydb::db::DatabaseConnection::connect(
        &imported.profile,
        imported.transient_password.as_ref(),
    )
    .await
    .expect("connect to configured ClickHouse test server");
    let profile_id = imported.profile.id;
    let database_name = database.probe().await.unwrap().database;
    let suffix = Uuid::new_v4().simple().to_string();
    let source_name = format!("lazydb_mv_source_{suffix}");
    let target_name = format!("lazydb_mv_target_{suffix}");
    let view_name = format!("lazydb_mv_{suffix}");
    database
        .execute(&format!(
            "CREATE TABLE `{database_name}`.`{source_name}` (id UInt64) ENGINE = MergeTree ORDER BY id"
        ))
        .await
        .expect("create isolated MV source table");
    database
        .execute(&format!(
            "CREATE TABLE `{database_name}`.`{target_name}` (id UInt64) ENGINE = MergeTree ORDER BY id"
        ))
        .await
        .expect("create isolated MV target table");
    let query = format!("SELECT id FROM `{database_name}`.`{source_name}`");
    database
        .execute(&format!(
            "CREATE MATERIALIZED VIEW `{database_name}`.`{view_name}` TO `{database_name}`.`{target_name}` AS {query}"
        ))
        .await
        .expect("create MV with explicit TO target and no POPULATE");
    database
        .execute(&format!(
            "INSERT INTO `{database_name}`.`{source_name}` VALUES (73)"
        ))
        .await
        .expect("insert into isolated MV source");
    let rows = database
        .execute(&format!(
            "SELECT count() FROM `{database_name}`.`{target_name}` WHERE id = 73"
        ))
        .await
        .expect("query MV target");
    assert_eq!(rows.result_sets[0].rows[0][0].display_text(16), "1");

    let object = CatalogId::new(
        profile_id,
        CatalogKind::MaterializedView,
        [
            database_name.as_str(),
            database_name.as_str(),
            view_name.as_str(),
        ],
    );
    let definition = database
        .load_catalog_object_definition(
            &lazydb::db::catalog_mutation::CatalogObjectDefinitionRequest {
                connection: identity(profile_id),
                request_id: 201,
                catalog_epoch: 1,
                object: object.clone(),
                target: lazydb::model::execution_target::ExecutionTarget {
                    profile_id,
                    database: database_name.clone(),
                    schema: Some(database_name.clone()),
                },
                principal: None,
            },
        )
        .await
        .expect("load MV native definition");
    let CatalogObjectDefinition::MaterializedView(definition) = definition else {
        panic!("native ClickHouse MV definition expected");
    };
    let native = definition.clickhouse_native.unwrap();
    assert!(native.query.contains("SELECT id"));
    assert!(native.query.contains(&source_name));
    assert_eq!(
        native.target.as_deref(),
        Some(format!("{database_name}.{target_name}").as_str())
    );
    assert!(native.canonical_ddl.to_ascii_uppercase().contains(" TO "));
    assert!(
        native
            .canonical_ddl
            .to_ascii_uppercase()
            .contains("AS SELECT")
    );
    assert!(
        !native
            .canonical_ddl
            .to_ascii_uppercase()
            .contains("POPULATE")
    );

    let entry = lazydb::db::catalog::CatalogEntry::relation(
        object.clone(),
        CatalogId::new(
            profile_id,
            CatalogKind::Schema,
            [database_name.as_str(), database_name.as_str()],
        ),
        lazydb::db::catalog::QualifiedName {
            database: Some(database_name.clone()),
            schema: Some(database_name.clone()),
            object: view_name,
        },
        "MaterializedView",
        lazydb::db::catalog::OptionalMetadata::Supported(None),
        true,
    )
    .unwrap();
    let drop_request =
        lazydb::db::catalog_drop::CatalogDropRequest::new(identity(profile_id), object, 202)
            .with_entry(entry.clone());
    let drop_plan = database.plan_catalog_drop(drop_request, &entry).unwrap();
    database
        .execute(drop_plan.sql())
        .await
        .expect("drop isolated MV through native drop planner");
    database
        .execute(&format!("DROP TABLE `{database_name}`.`{source_name}`"))
        .await
        .expect("drop isolated MV source table");
    database
        .execute(&format!("DROP TABLE `{database_name}`.`{target_name}`"))
        .await
        .expect("drop isolated MV target table");
    database.close().await;
}
