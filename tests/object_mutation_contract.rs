use lazydb::db::catalog::{CatalogId, CatalogKind, NamespaceModel};
use lazydb::db::catalog_change_set::{CatalogFieldChanges, FieldChange};
use lazydb::db::catalog_mutation::{
    CatalogMutationAnchor, CatalogMutationAvailability, CatalogMutationCapabilities,
    CatalogMutationMode, CatalogMutationOption, CatalogObjectType, MutationCompletion,
    MutationProgress,
};
use lazydb::db::catalog_mutation::{CatalogRebuildPlan, CatalogRebuildStep};
use lazydb::db::mssql::MsSqlAdapter;
use lazydb::db::mysql::MySqlAdapter;
use lazydb::db::oracle::OracleAdapter;
use lazydb::db::sqlite::SqliteAdapter;
use lazydb::model::catalog_editor::{CatalogDraft, TableDraft, ViewDraft};
use lazydb::model::explorer::ExplorerNodeId;
use lazydb::model::explorer_actions::{ExplorerActionAvailability, ExplorerActionContext};
use lazydb::profile::DatabaseKind;
use uuid::Uuid;

fn available(object_type: CatalogObjectType) -> CatalogMutationOption {
    CatalogMutationOption {
        object_type,
        availability: CatalogMutationAvailability::Available,
    }
}

#[test]
fn capabilities_distinguish_available_unavailable_and_unimplemented_operations() {
    let capabilities = CatalogMutationCapabilities {
        create: vec![available(CatalogObjectType::Catalog(CatalogKind::Schema))],
        ..CatalogMutationCapabilities::default()
    };

    assert_eq!(
        capabilities.create_availability(CatalogObjectType::Catalog(CatalogKind::Schema)),
        Some(CatalogMutationAvailability::Available)
    );
    assert_eq!(
        capabilities.create_availability(CatalogObjectType::Catalog(CatalogKind::Table)),
        None,
        "an operation not advertised by an adapter must not be treated as available"
    );
}

#[test]
fn default_capabilities_do_not_accidentally_expose_mutations() {
    let capabilities = CatalogMutationCapabilities::default();

    assert!(capabilities.profile_create.is_empty());
    assert!(capabilities.create.is_empty());
    assert!(capabilities.edit.is_empty());
}

#[test]
fn database_is_schema_does_not_offer_a_phantom_schema_creation() {
    let capabilities = CatalogMutationCapabilities {
        create: vec![
            available(CatalogObjectType::Catalog(CatalogKind::Table)),
            available(CatalogObjectType::Catalog(CatalogKind::View)),
        ],
        ..CatalogMutationCapabilities::default()
    };
    let database = CatalogId::new(
        Uuid::from_u128(1),
        CatalogKind::Database,
        vec!["app".to_owned()],
    );

    let options = capabilities
        .create_options_for_namespace(
            &CatalogMutationAnchor::Catalog(database),
            None,
            NamespaceModel::DatabaseIsSchema,
        )
        .expect("valid database anchor");

    assert_eq!(
        options,
        vec![
            CatalogObjectType::Catalog(CatalogKind::Table),
            CatalogObjectType::Catalog(CatalogKind::View),
        ]
    );
}

#[test]
fn explorer_action_resolution_returns_a_reason_instead_of_silently_dropping_a_key() {
    let capabilities = CatalogMutationCapabilities::default();
    let context = ExplorerActionContext {
        database_kind: DatabaseKind::Oracle,
        connected: true,
        read_only: false,
        namespace_model: NamespaceModel::DatabaseAndSchema,
        capabilities: &capabilities,
        selected_entry: None,
    };
    let selected = ExplorerNodeId::Profile(Uuid::from_u128(1));

    assert!(matches!(
        context.resolve(Some(&selected), CatalogMutationMode::Create),
        ExplorerActionAvailability::Unavailable(reason) if reason.contains("Oracle")
    ));
}

#[test]
fn mutation_change_sets_preserve_unknown_and_unchanged_fields() {
    let changes = CatalogFieldChanges::<String> {
        name: FieldChange::changed("renamed".to_owned()),
        comment: FieldChange::Unknown,
    };

    assert!(changes.has_changes());
    assert_eq!(
        changes.name.as_changed().map(String::as_str),
        Some("renamed")
    );
    assert_eq!(changes.comment.as_changed(), None);
}

#[test]
fn mutation_progress_distinguishes_rollback_partial_apply_and_unknown_commit() {
    assert_eq!(
        MutationProgress::failed(1, vec![0]).completion,
        MutationCompletion::Failed
    );
    assert_eq!(
        MutationProgress::partially_applied(1, vec![0]).completion,
        MutationCompletion::PartiallyApplied
    );
    assert_eq!(
        MutationProgress::outcome_unknown(vec![0]).completion,
        MutationCompletion::OutcomeUnknown
    );
    assert_eq!(MutationProgress::succeeded(2).completed_steps, vec![0, 1]);
}

#[test]
fn oracle_advertises_only_the_object_groups_with_creation_plans() {
    let capabilities = OracleAdapter::catalog_mutation_capabilities();
    assert!(
        capabilities
            .create_availability(CatalogObjectType::Catalog(CatalogKind::Table))
            .is_some()
    );
    assert!(
        capabilities
            .create_availability(CatalogObjectType::Catalog(CatalogKind::View))
            .is_some()
    );
    assert!(
        capabilities
            .create_availability(CatalogObjectType::Catalog(CatalogKind::Sequence))
            .is_some()
    );
    assert_eq!(capabilities.edit.len(), 3);
}

#[test]
fn oracle_advertises_editing_only_for_loaded_definition_types() {
    let capabilities = OracleAdapter::catalog_mutation_capabilities();
    assert!(capabilities.edit.iter().all(|option| matches!(
        option.object_type,
        CatalogObjectType::Catalog(CatalogKind::Table | CatalogKind::View | CatalogKind::Sequence)
    )));
}

#[test]
fn oracle_table_edit_plan_renames_only_the_table_identity() {
    let profile_id = Uuid::from_u128(9);
    let object = CatalogId::new(profile_id, CatalogKind::Table, ["SERVICE", "APP", "OLD"]);
    let request = lazydb::db::catalog_mutation::CatalogMutationRequest {
        connection: lazydb::identity::ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        request_id: 2,
        catalog_epoch: 3,
        mode: CatalogMutationMode::Edit,
        anchor: CatalogMutationAnchor::Catalog(object),
        object_type: CatalogObjectType::Catalog(CatalogKind::Table),
        current_database: Some("SERVICE".to_owned()),
    };
    let mut table = TableDraft::new("APP");
    table.name.set("NEW");
    let baseline = lazydb::db::catalog_mutation::CatalogObjectDefinition::Table(
        lazydb::db::catalog_mutation::TableDefinition {
            database: "SERVICE".into(),
            schema: "APP".into(),
            name: "OLD".into(),
            owner: "APP".into(),
            comment: lazydb::db::catalog::OptionalMetadata::Unsupported,
            columns: vec![],
            indexes: vec![],
            constraints: vec![],
            structured_constraints: vec![],
            baseline_fingerprint: "old".into(),
        },
    );
    let plan =
        OracleAdapter::plan_catalog_mutation(request, CatalogDraft::Table(table), Some(baseline))
            .expect("Oracle rename plan should be valid");
    assert_eq!(
        plan.statements(),
        &["ALTER TABLE \"APP\".\"OLD\" RENAME TO \"NEW\""]
    );
}

#[test]
fn mysql_table_and_column_comment_edits_produce_mutation_sql() {
    let profile_id = Uuid::from_u128(90);
    let object = CatalogId::new(profile_id, CatalogKind::Table, ["app", "app", "items"]);
    let request = lazydb::db::catalog_mutation::CatalogMutationRequest {
        connection: lazydb::identity::ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        request_id: 1,
        catalog_epoch: 1,
        mode: CatalogMutationMode::Edit,
        anchor: CatalogMutationAnchor::Catalog(object),
        object_type: CatalogObjectType::Catalog(CatalogKind::Table),
        current_database: Some("app".into()),
    };
    let definition = lazydb::db::catalog_mutation::TableDefinition {
        database: "app".into(),
        schema: "app".into(),
        name: "items".into(),
        owner: String::new(),
        comment: lazydb::db::catalog::OptionalMetadata::Supported(Some("old table".into())),
        columns: vec![lazydb::db::catalog_mutation::ColumnDefinition {
            name: "id".into(),
            ordinal_position: 1,
            native_type: "INT".into(),
            nullable: false,
            default_expression: lazydb::db::catalog::OptionalMetadata::Supported(None),
            identity: lazydb::db::catalog::OptionalMetadata::Supported(Some(true)),
            generated_expression: lazydb::db::catalog::OptionalMetadata::Supported(None),
            collation: lazydb::db::catalog::OptionalMetadata::Supported(None),
            comment: lazydb::db::catalog::OptionalMetadata::Supported(Some("old column".into())),
        }],
        indexes: vec![],
        constraints: vec![],
        structured_constraints: vec![],
        baseline_fingerprint: "items".into(),
    };
    let mut draft = TableDraft::from_definition_for_database(&definition, DatabaseKind::MySql);
    draft.comment.set("new table");
    draft.columns[0].comment.set("new column");
    let plan = MySqlAdapter::plan_catalog_mutation(
        request,
        CatalogDraft::Table(draft),
        Some(lazydb::db::catalog_mutation::CatalogObjectDefinition::Table(definition)),
    )
    .expect("comment-only MySQL edit should produce a plan");
    assert!(
        plan.statements()
            .iter()
            .any(|sql| sql.contains("ALTER TABLE `app`.`items` COMMENT = 'new table'"))
    );
    assert!(
        plan.statements().iter().any(|sql| {
            sql.contains("CHANGE COLUMN `id`") && sql.contains("COMMENT 'new column'")
        })
    );
}

#[test]
fn oracle_table_and_column_comment_edits_produce_comment_statements() {
    let profile_id = Uuid::from_u128(91);
    let object = CatalogId::new(profile_id, CatalogKind::Table, ["SERVICE", "APP", "ITEMS"]);
    let request = lazydb::db::catalog_mutation::CatalogMutationRequest {
        connection: lazydb::identity::ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        request_id: 1,
        catalog_epoch: 1,
        mode: CatalogMutationMode::Edit,
        anchor: CatalogMutationAnchor::Catalog(object),
        object_type: CatalogObjectType::Catalog(CatalogKind::Table),
        current_database: Some("SERVICE".into()),
    };
    let definition = lazydb::db::catalog_mutation::TableDefinition {
        database: "SERVICE".into(),
        schema: "APP".into(),
        name: "ITEMS".into(),
        owner: "APP".into(),
        comment: lazydb::db::catalog::OptionalMetadata::Supported(Some("old table".into())),
        columns: vec![lazydb::db::catalog_mutation::ColumnDefinition {
            name: "ID".into(),
            ordinal_position: 1,
            native_type: "NUMBER".into(),
            nullable: false,
            default_expression: lazydb::db::catalog::OptionalMetadata::Supported(None),
            identity: lazydb::db::catalog::OptionalMetadata::Unsupported,
            generated_expression: lazydb::db::catalog::OptionalMetadata::Unsupported,
            collation: lazydb::db::catalog::OptionalMetadata::Unsupported,
            comment: lazydb::db::catalog::OptionalMetadata::Supported(Some("old column".into())),
        }],
        indexes: vec![],
        constraints: vec![],
        structured_constraints: vec![],
        baseline_fingerprint: "items".into(),
    };
    let mut draft = TableDraft::from_definition(&definition);
    draft.comment.set("new table");
    draft.columns[0].comment.set("new column");
    let plan = OracleAdapter::plan_catalog_mutation(
        request,
        CatalogDraft::Table(draft),
        Some(lazydb::db::catalog_mutation::CatalogObjectDefinition::Table(definition)),
    )
    .expect("comment-only Oracle edit should produce a plan");
    assert_eq!(
        plan.statements(),
        &[
            "COMMENT ON TABLE \"APP\".\"ITEMS\" IS 'new table'",
            "COMMENT ON COLUMN \"APP\".\"ITEMS\".\"ID\" IS 'new column'",
        ]
    );
    assert!(!plan.impact.native_identity_changed);
}

#[test]
fn comment_edits_preserve_no_changes_and_support_clearing() {
    let profile_id = Uuid::from_u128(92);
    let object = CatalogId::new(profile_id, CatalogKind::Table, ["app", "app", "items"]);
    let request = |request_id| lazydb::db::catalog_mutation::CatalogMutationRequest {
        connection: lazydb::identity::ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        request_id,
        catalog_epoch: 1,
        mode: CatalogMutationMode::Edit,
        anchor: CatalogMutationAnchor::Catalog(object.clone()),
        object_type: CatalogObjectType::Catalog(CatalogKind::Table),
        current_database: Some("app".into()),
    };
    let definition = lazydb::db::catalog_mutation::TableDefinition {
        database: "app".into(),
        schema: "app".into(),
        name: "items".into(),
        owner: String::new(),
        comment: lazydb::db::catalog::OptionalMetadata::Supported(Some("table".into())),
        columns: vec![lazydb::db::catalog_mutation::ColumnDefinition {
            name: "id".into(),
            ordinal_position: 1,
            native_type: "INT".into(),
            nullable: false,
            default_expression: lazydb::db::catalog::OptionalMetadata::Supported(None),
            identity: lazydb::db::catalog::OptionalMetadata::Supported(Some(false)),
            generated_expression: lazydb::db::catalog::OptionalMetadata::Supported(None),
            collation: lazydb::db::catalog::OptionalMetadata::Supported(None),
            comment: lazydb::db::catalog::OptionalMetadata::Supported(Some("column".into())),
        }],
        indexes: vec![],
        constraints: vec![],
        structured_constraints: vec![],
        baseline_fingerprint: "items".into(),
    };
    let baseline = lazydb::db::catalog_mutation::CatalogObjectDefinition::Table(definition.clone());
    let unchanged = TableDraft::from_definition_for_database(&definition, DatabaseKind::MySql);
    let unchanged_result = MySqlAdapter::plan_catalog_mutation(
        request(1),
        CatalogDraft::Table(unchanged),
        Some(baseline.clone()),
    );
    assert!(matches!(
        unchanged_result,
        Err(lazydb::db::catalog_mutation::CatalogMutationError::NoChanges)
    ));

    let mut clear = TableDraft::from_definition_for_database(&definition, DatabaseKind::MySql);
    clear.comment.clear();
    clear.columns[0].comment.clear();
    let plan =
        MySqlAdapter::plan_catalog_mutation(request(2), CatalogDraft::Table(clear), Some(baseline))
            .expect("clearing comments should produce a plan");
    assert!(
        plan.statements()
            .iter()
            .any(|sql| sql.ends_with("COMMENT = ''"))
    );
    assert!(
        plan.statements()
            .iter()
            .any(|sql| sql.contains("COMMENT ''"))
    );
}

#[test]
fn oracle_view_edit_plan_replaces_the_definition() {
    let profile_id = Uuid::from_u128(10);
    let object = CatalogId::new(profile_id, CatalogKind::View, ["SERVICE", "APP", "V"]);
    let request = lazydb::db::catalog_mutation::CatalogMutationRequest {
        connection: lazydb::identity::ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        request_id: 3,
        catalog_epoch: 4,
        mode: CatalogMutationMode::Edit,
        anchor: CatalogMutationAnchor::Catalog(object),
        object_type: CatalogObjectType::Catalog(CatalogKind::View),
        current_database: Some("SERVICE".to_owned()),
    };
    let mut view = lazydb::model::catalog_editor::ViewDraft {
        name: "V".into(),
        schema: "APP".into(),
        owner: "APP".into(),
        comment: Default::default(),
        query: "SELECT 1 FROM dual".into(),
        output_columns: Default::default(),
        security_barrier: lazydb::db::catalog_mutation::ViewOption::unavailable("not applicable"),
        security_invoker: lazydb::db::catalog_mutation::ViewOption::unavailable("not applicable"),
        check_option: lazydb::db::catalog_mutation::ViewOption::unavailable("not applicable"),
        focus: lazydb::model::catalog_editor::CatalogFormFocus::Name,
    };
    view.query.set("SELECT 2 FROM dual");
    let baseline = lazydb::db::catalog_mutation::CatalogObjectDefinition::View(
        lazydb::db::catalog_mutation::ViewDefinition {
            database: "SERVICE".into(),
            schema: "APP".into(),
            name: "V".into(),
            owner: "APP".into(),
            comment: lazydb::db::catalog::OptionalMetadata::Unsupported,
            query: "SELECT 1 FROM dual".into(),
            output_columns: vec![],
            security_barrier: lazydb::db::catalog_mutation::ViewOption::unavailable(
                "not applicable",
            ),
            security_invoker: lazydb::db::catalog_mutation::ViewOption::unavailable(
                "not applicable",
            ),
            check_option: lazydb::db::catalog_mutation::ViewOption::unavailable("not applicable"),
            baseline_fingerprint: "old".into(),
        },
    );
    let plan =
        OracleAdapter::plan_catalog_mutation(request, CatalogDraft::View(view), Some(baseline))
            .expect("Oracle view replace plan should be valid");
    assert_eq!(
        plan.statements(),
        &["CREATE OR REPLACE VIEW \"APP\".\"V\" AS SELECT 2 FROM dual"]
    );
}

#[test]
fn oracle_sequence_edit_plan_updates_runtime_attributes() {
    let profile_id = Uuid::from_u128(11);
    let object = CatalogId::new(profile_id, CatalogKind::Sequence, ["SERVICE", "APP", "SEQ"]);
    let request = lazydb::db::catalog_mutation::CatalogMutationRequest {
        connection: lazydb::identity::ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        request_id: 4,
        catalog_epoch: 5,
        mode: CatalogMutationMode::Edit,
        anchor: CatalogMutationAnchor::Catalog(object),
        object_type: CatalogObjectType::Catalog(CatalogKind::Sequence),
        current_database: Some("SERVICE".to_owned()),
    };
    let sequence = lazydb::model::catalog_editor::SequenceDraft {
        name: "SEQ".into(),
        schema: "APP".into(),
        owner: "APP".into(),
        comment: Default::default(),
        data_type: "NUMBER".into(),
        increment: "5".into(),
        min_value: lazydb::model::catalog_editor::SequenceBoundDraft::from(
            lazydb::db::catalog_mutation::SequenceBound::Unset,
        ),
        max_value: lazydb::model::catalog_editor::SequenceBoundDraft::from(
            lazydb::db::catalog_mutation::SequenceBound::Unset,
        ),
        start_value: "1".into(),
        restart_value: Default::default(),
        cache: "20".into(),
        cycle: true,
        owned_by: "NONE".into(),
        focus: lazydb::model::catalog_editor::CatalogFormFocus::Name,
    };
    sequence.validate().unwrap();
    let baseline = lazydb::db::catalog_mutation::CatalogObjectDefinition::Sequence(
        lazydb::db::catalog_mutation::SequenceDefinition {
            database: "SERVICE".into(),
            schema: "APP".into(),
            name: "SEQ".into(),
            owner: "APP".into(),
            comment: lazydb::db::catalog::OptionalMetadata::Unsupported,
            data_type: "NUMBER".into(),
            increment: "1".into(),
            min_value: lazydb::db::catalog_mutation::SequenceBound::Unset,
            max_value: lazydb::db::catalog_mutation::SequenceBound::Unset,
            start_value: "1".into(),
            cache: "1".into(),
            cycle: false,
            owned_by: None,
            baseline_fingerprint: "old".into(),
        },
    );
    let plan = OracleAdapter::plan_catalog_mutation(
        request,
        CatalogDraft::Sequence(sequence),
        Some(baseline),
    )
    .expect("Oracle sequence edit plan should be valid");
    assert_eq!(
        plan.statements(),
        &["ALTER SEQUENCE \"APP\".\"SEQ\" INCREMENT BY 5 CACHE 20 CYCLE"]
    );
}

#[test]
fn mysql_database_is_schema_advertises_table_and_view_creation() {
    let capabilities = MySqlAdapter::catalog_mutation_capabilities();
    assert!(
        capabilities
            .profile_create
            .iter()
            .any(|option| option.object_type == CatalogObjectType::Catalog(CatalogKind::Database))
    );
    assert!(
        capabilities
            .create_availability(CatalogObjectType::Catalog(CatalogKind::Table))
            .is_some()
    );
    assert!(
        capabilities
            .create_availability(CatalogObjectType::Catalog(CatalogKind::View))
            .is_some()
    );
    assert!(
        capabilities
            .create_availability(CatalogObjectType::Catalog(CatalogKind::Schema))
            .is_none()
    );
    assert_eq!(capabilities.edit.len(), 2);
}

#[test]
fn mysql_table_create_plan_uses_backtick_quoting() {
    let profile_id = Uuid::from_u128(8);
    let request = lazydb::db::catalog_mutation::CatalogMutationRequest {
        connection: lazydb::identity::ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        request_id: 1,
        catalog_epoch: 1,
        mode: CatalogMutationMode::Create,
        anchor: CatalogMutationAnchor::Group {
            schema: CatalogId::new(profile_id, CatalogKind::Schema, ["shop`db", "shop`db"]),
            group: lazydb::db::catalog::ObjectGroup::Tables,
        },
        object_type: CatalogObjectType::Catalog(CatalogKind::Table),
        current_database: Some("shop`db".to_owned()),
    };
    let mut table = TableDraft::new("shop`db");
    table.name.set("line`item");
    table.columns[0].name.set("id");
    table.columns[0].native_type.set("BIGINT");
    let plan = MySqlAdapter::plan_catalog_mutation(request, CatalogDraft::Table(table), None)
        .expect("MySQL table plan should be valid");
    assert_eq!(
        plan.statements()[0],
        "CREATE TABLE `shop``db`.`line``item` (`id` BIGINT)"
    );
}

#[test]
fn mysql_database_anchor_creates_a_table_in_the_database_namespace() {
    let profile_id = Uuid::from_u128(18);
    let request = lazydb::db::catalog_mutation::CatalogMutationRequest::new(
        lazydb::identity::ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        1,
        1,
        CatalogMutationMode::Create,
        CatalogMutationAnchor::Catalog(CatalogId::new(profile_id, CatalogKind::Database, ["app"])),
        CatalogObjectType::Catalog(CatalogKind::Table),
    )
    .unwrap()
    .with_current_database("app");
    let mut table = TableDraft::new_for_database("app", DatabaseKind::MariaDb);
    table.name.set("events");
    table.columns[0].name.set("id");
    table.columns[0].native_type.set("INT");
    let plan = MySqlAdapter::plan_catalog_mutation(request, CatalogDraft::Table(table), None)
        .expect("database anchor should create a table plan");

    assert_eq!(
        plan.statements()[0],
        "CREATE TABLE `app`.`events` (`id` INT)"
    );
    assert!(matches!(
        plan.refresh.as_slice(),
        [lazydb::db::catalog::CatalogTarget::Objects {
            group: lazydb::db::catalog::ObjectGroup::Tables,
            ..
        }]
    ));
}

#[test]
fn mysql_profile_anchor_creates_a_database_on_an_existing_target() {
    let profile_id = Uuid::from_u128(19);
    let request = lazydb::db::catalog_mutation::CatalogMutationRequest::new(
        lazydb::identity::ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        1,
        1,
        CatalogMutationMode::Create,
        CatalogMutationAnchor::Profile { profile_id },
        CatalogObjectType::Catalog(CatalogKind::Database),
    )
    .unwrap()
    .with_current_database("mysql");
    let mut database = lazydb::model::catalog_editor::DatabaseDraft::new("");
    database.database_kind = DatabaseKind::MariaDb;
    database.name.set("reporting");
    let plan = MySqlAdapter::plan_catalog_mutation(request, CatalogDraft::Database(database), None)
        .expect("profile anchor should create a database plan");

    assert_eq!(plan.statements()[0], "CREATE DATABASE `reporting`");
    assert_eq!(plan.execution_target.database(), "mysql");
    assert!(matches!(
        plan.refresh.as_slice(),
        [lazydb::db::catalog::CatalogTarget::Databases]
    ));
}

#[test]
fn mysql_view_edit_plan_renames_and_replaces_the_definition() {
    let profile_id = Uuid::from_u128(12);
    let object = CatalogId::new(profile_id, CatalogKind::View, ["shop", "shop", "old_view"]);
    let request = lazydb::db::catalog_mutation::CatalogMutationRequest {
        connection: lazydb::identity::ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        request_id: 2,
        catalog_epoch: 1,
        mode: CatalogMutationMode::Edit,
        anchor: CatalogMutationAnchor::Catalog(object),
        object_type: CatalogObjectType::Catalog(CatalogKind::View),
        current_database: Some("shop".to_owned()),
    };
    let view = ViewDraft {
        name: "new_view".into(),
        schema: "shop".into(),
        owner: "".into(),
        comment: "".into(),
        query: "SELECT 2".into(),
        output_columns: "".into(),
        security_barrier: lazydb::db::catalog_mutation::ViewOption::unavailable(
            "not applicable to MySQL",
        ),
        security_invoker: lazydb::db::catalog_mutation::ViewOption::unavailable(
            "not applicable to MySQL",
        ),
        check_option: lazydb::db::catalog_mutation::ViewOption::unavailable("not mapped for MySQL"),
        focus: lazydb::model::catalog_editor::CatalogFormFocus::Name,
    };
    let baseline = lazydb::db::catalog_mutation::CatalogObjectDefinition::View(
        lazydb::db::catalog_mutation::ViewDefinition {
            database: "shop".into(),
            schema: "shop".into(),
            name: "old_view".into(),
            owner: "".into(),
            comment: lazydb::db::catalog::OptionalMetadata::Unsupported,
            query: "SELECT 1".into(),
            output_columns: vec!["value".into()],
            security_barrier: lazydb::db::catalog_mutation::ViewOption::unavailable(
                "not applicable to MySQL",
            ),
            security_invoker: lazydb::db::catalog_mutation::ViewOption::unavailable(
                "not applicable to MySQL",
            ),
            check_option: lazydb::db::catalog_mutation::ViewOption::unavailable(
                "not mapped for MySQL",
            ),
            baseline_fingerprint: "old".into(),
        },
    );
    let plan =
        MySqlAdapter::plan_catalog_mutation(request, CatalogDraft::View(view), Some(baseline))
            .expect("MySQL view edit plan should be valid");
    assert_eq!(
        plan.statements(),
        &[
            "RENAME TABLE `shop`.`old_view` TO `shop`.`new_view`",
            "CREATE OR REPLACE VIEW `shop`.`new_view` AS SELECT 2"
        ]
    );
}

#[test]
fn mysql_compatible_catalog_version_gates_remain_engine_specific() {
    assert!(lazydb::db::mysql::supports_catalog_version_for_kind(
        DatabaseKind::MySql,
        "8.0.13"
    ));
    assert!(!lazydb::db::mysql::supports_catalog_version_for_kind(
        DatabaseKind::MySql,
        "10.5.0-MariaDB"
    ));
    assert!(lazydb::db::mysql::supports_catalog_version_for_kind(
        DatabaseKind::MariaDb,
        "10.5.0-MariaDB"
    ));
    assert!(!lazydb::db::mysql::supports_catalog_version_for_kind(
        DatabaseKind::MariaDb,
        "8.0.13"
    ));
}

#[test]
fn sqlite_exposes_only_native_table_and_view_creation() {
    let capabilities = SqliteAdapter::catalog_mutation_capabilities();
    assert!(
        capabilities
            .create_availability(CatalogObjectType::Catalog(CatalogKind::Table))
            .is_some()
    );
    assert!(
        capabilities
            .create_availability(CatalogObjectType::Catalog(CatalogKind::View))
            .is_some()
    );
    assert!(
        capabilities
            .create_availability(CatalogObjectType::Catalog(CatalogKind::Schema))
            .is_none()
    );
    assert_eq!(capabilities.edit.len(), 2);
}

#[test]
fn sqlite_table_edit_plan_uses_native_rename() {
    let profile_id = Uuid::from_u128(15);
    let object = CatalogId::new(profile_id, CatalogKind::Table, [":memory:", "main", "old"]);
    let request = lazydb::db::catalog_mutation::CatalogMutationRequest {
        connection: lazydb::identity::ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        request_id: 5,
        catalog_epoch: 1,
        mode: CatalogMutationMode::Edit,
        anchor: CatalogMutationAnchor::Catalog(object),
        object_type: CatalogObjectType::Catalog(CatalogKind::Table),
        current_database: Some(":memory:".to_owned()),
    };
    let mut table = TableDraft::new("main");
    table.name.set("new");
    table.columns[0].name.set("id");
    table.columns[0].native_type.set("INTEGER");
    table.columns[0].existing_name = Some("id".into());
    table.columns[0].state = lazydb::model::catalog_editor::DraftRowState::Existing {
        id: CatalogId::new(profile_id, CatalogKind::Column, ["id"]),
    };
    let baseline = lazydb::db::catalog_mutation::CatalogObjectDefinition::Table(
        lazydb::db::catalog_mutation::TableDefinition {
            database: ":memory:".into(),
            schema: "main".into(),
            name: "old".into(),
            owner: String::new(),
            comment: lazydb::db::catalog::OptionalMetadata::Unsupported,
            columns: vec![lazydb::db::catalog_mutation::ColumnDefinition {
                name: "id".into(),
                ordinal_position: 1,
                native_type: "INTEGER".into(),
                nullable: true,
                default_expression: lazydb::db::catalog::OptionalMetadata::Unsupported,
                identity: lazydb::db::catalog::OptionalMetadata::Unsupported,
                generated_expression: lazydb::db::catalog::OptionalMetadata::Unsupported,
                collation: lazydb::db::catalog::OptionalMetadata::Unsupported,
                comment: lazydb::db::catalog::OptionalMetadata::Unsupported,
            }],
            indexes: vec![],
            constraints: vec![],
            structured_constraints: vec![],
            baseline_fingerprint: "old".into(),
        },
    );
    let plan =
        SqliteAdapter::plan_catalog_mutation(request, CatalogDraft::Table(table), Some(baseline))
            .expect("SQLite table rename plan should be valid");
    assert_eq!(
        plan.statements(),
        &["ALTER TABLE \"main\".\"old\" RENAME TO \"new\""]
    );
}

#[test]
fn sql_server_exposes_native_table_and_view_creation_and_editing() {
    let capabilities = MsSqlAdapter::catalog_mutation_capabilities();
    assert!(
        capabilities
            .create
            .iter()
            .any(|option| { option.object_type == CatalogObjectType::Catalog(CatalogKind::Table) })
    );
    assert!(
        capabilities
            .create
            .iter()
            .any(|option| { option.object_type == CatalogObjectType::Catalog(CatalogKind::View) })
    );
    assert_eq!(capabilities.edit.len(), 2);
}

#[test]
fn sql_server_table_edit_plan_uses_sp_rename() {
    let profile_id = Uuid::from_u128(13);
    let object = CatalogId::new(
        profile_id,
        CatalogKind::Table,
        ["app", "dbo", "old_table", "42"],
    );
    let request = lazydb::db::catalog_mutation::CatalogMutationRequest {
        connection: lazydb::identity::ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        request_id: 3,
        catalog_epoch: 1,
        mode: CatalogMutationMode::Edit,
        anchor: CatalogMutationAnchor::Catalog(object),
        object_type: CatalogObjectType::Catalog(CatalogKind::Table),
        current_database: Some("app".to_owned()),
    };
    let mut table = TableDraft::new("dbo");
    table.name.set("new_table");
    let baseline = lazydb::db::catalog_mutation::CatalogObjectDefinition::Table(
        lazydb::db::catalog_mutation::TableDefinition {
            database: "app".into(),
            schema: "dbo".into(),
            name: "old_table".into(),
            owner: "dbo".into(),
            comment: lazydb::db::catalog::OptionalMetadata::Unsupported,
            columns: vec![],
            indexes: vec![],
            constraints: vec![],
            structured_constraints: vec![],
            baseline_fingerprint: "old".into(),
        },
    );
    let plan =
        MsSqlAdapter::plan_catalog_mutation(request, CatalogDraft::Table(table), Some(baseline))
            .expect("SQL Server table edit plan should be valid");
    assert_eq!(
        plan.statements(),
        &["EXEC sys.sp_rename N'[dbo].[old_table]', N'new_table', N'OBJECT'"]
    );
}

#[test]
fn sql_server_view_edit_plan_renames_and_alters_the_definition() {
    let profile_id = Uuid::from_u128(14);
    let object = CatalogId::new(
        profile_id,
        CatalogKind::View,
        ["app", "dbo", "old_view", "43"],
    );
    let request = lazydb::db::catalog_mutation::CatalogMutationRequest {
        connection: lazydb::identity::ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        request_id: 4,
        catalog_epoch: 1,
        mode: CatalogMutationMode::Edit,
        anchor: CatalogMutationAnchor::Catalog(object),
        object_type: CatalogObjectType::Catalog(CatalogKind::View),
        current_database: Some("app".to_owned()),
    };
    let view = ViewDraft {
        name: "new_view".into(),
        schema: "dbo".into(),
        owner: "dbo".into(),
        comment: "".into(),
        query: "SELECT 2".into(),
        output_columns: "".into(),
        security_barrier: lazydb::db::catalog_mutation::ViewOption::unavailable(
            "not applicable to SQL Server",
        ),
        security_invoker: lazydb::db::catalog_mutation::ViewOption::unavailable(
            "not applicable to SQL Server",
        ),
        check_option: lazydb::db::catalog_mutation::ViewOption::unavailable(
            "not mapped for SQL Server",
        ),
        focus: lazydb::model::catalog_editor::CatalogFormFocus::Name,
    };
    let baseline = lazydb::db::catalog_mutation::CatalogObjectDefinition::View(
        lazydb::db::catalog_mutation::ViewDefinition {
            database: "app".into(),
            schema: "dbo".into(),
            name: "old_view".into(),
            owner: "dbo".into(),
            comment: lazydb::db::catalog::OptionalMetadata::Unsupported,
            query: "SELECT 1".into(),
            output_columns: vec!["value".into()],
            security_barrier: lazydb::db::catalog_mutation::ViewOption::unavailable(
                "not applicable to SQL Server",
            ),
            security_invoker: lazydb::db::catalog_mutation::ViewOption::unavailable(
                "not applicable to SQL Server",
            ),
            check_option: lazydb::db::catalog_mutation::ViewOption::unavailable(
                "not mapped for SQL Server",
            ),
            baseline_fingerprint: "old".into(),
        },
    );
    let plan =
        MsSqlAdapter::plan_catalog_mutation(request, CatalogDraft::View(view), Some(baseline))
            .expect("SQL Server view edit plan should be valid");
    assert_eq!(
        plan.statements(),
        &[
            "EXEC sys.sp_rename N'[dbo].[old_view]', N'new_view', N'OBJECT'",
            "ALTER VIEW [dbo].[new_view] AS SELECT 2"
        ]
    );
}

#[test]
fn sqlite_rebuild_plan_requires_lossless_ordered_steps() {
    let plan = CatalogRebuildPlan {
        steps: vec![
            CatalogRebuildStep::CreateReplacement,
            CatalogRebuildStep::CopyRows {
                column_mapping: vec![("old".to_owned(), "new".to_owned())],
            },
            CatalogRebuildStep::DropOriginal,
            CatalogRebuildStep::RenameReplacement,
            CatalogRebuildStep::RestoreIndexes,
            CatalogRebuildStep::RestoreTriggers,
            CatalogRebuildStep::Validate,
        ],
        preserves_data: true,
        preserves_indexes: true,
        preserves_triggers: true,
    };
    assert!(plan.validate().is_ok());
}

#[test]
fn oracle_create_plans_quote_names_and_target_the_selected_group() {
    let profile_id = Uuid::from_u128(7);
    let schema = CatalogId::new(profile_id, CatalogKind::Schema, ["SERVICE", "APP"]);
    let connection = lazydb::identity::ConnectionIdentity {
        profile_id,
        generation: 1,
    };
    let request = lazydb::db::catalog_mutation::CatalogMutationRequest {
        connection,
        request_id: 1,
        catalog_epoch: 1,
        mode: CatalogMutationMode::Create,
        anchor: CatalogMutationAnchor::Group {
            schema: schema.clone(),
            group: lazydb::db::catalog::ObjectGroup::Tables,
        },
        object_type: CatalogObjectType::Catalog(CatalogKind::Table),
        current_database: Some("SERVICE".to_owned()),
    };
    let mut table = TableDraft::new_for_database("APP", DatabaseKind::Oracle);
    table.name.set("Order\"Items");
    table.columns[0].name.set("id");
    table.columns[0].native_type.set("NUMBER");
    table.columns[0].nullable = false;
    table.columns[0].default_expression.set("1");
    let plan = OracleAdapter::plan_catalog_mutation(request, CatalogDraft::Table(table), None)
        .expect("Oracle table plan should be valid");
    assert_eq!(
        plan.statements()[0],
        "CREATE TABLE \"APP\".\"Order\"\"Items\" (\"id\" NUMBER DEFAULT 1 NOT NULL)"
    );
    assert!(lazydb::sql::oracle::prepare_oracle_statement(&plan.statements()[0]).is_ok());
    assert_eq!(plan.step_count(), 1);
}

#[test]
fn oracle_create_uses_a_valid_default_type_for_new_columns() {
    let profile_id = Uuid::from_u128(8);
    let schema = CatalogId::new(profile_id, CatalogKind::Schema, ["SERVICE", "APP"]);
    let connection = lazydb::identity::ConnectionIdentity {
        profile_id,
        generation: 1,
    };
    let request = lazydb::db::catalog_mutation::CatalogMutationRequest {
        connection,
        request_id: 2,
        catalog_epoch: 1,
        mode: CatalogMutationMode::Create,
        anchor: CatalogMutationAnchor::Group {
            schema,
            group: lazydb::db::catalog::ObjectGroup::Tables,
        },
        object_type: CatalogObjectType::Catalog(CatalogKind::Table),
        current_database: Some("SERVICE".to_owned()),
    };
    let mut table = TableDraft::new_for_database("APP", DatabaseKind::Oracle);
    table.name.set("new_table");
    table.columns[0].name.set("name");

    let plan = OracleAdapter::plan_catalog_mutation(request, CatalogDraft::Table(table), None)
        .expect("Oracle table plan should be valid with the default column type");

    assert_eq!(
        plan.statements()[0],
        "CREATE TABLE \"APP\".\"new_table\" (\"name\" VARCHAR2(255 CHAR))"
    );
}

#[test]
fn oracle_create_rejects_text_before_execution_with_a_replacement_hint() {
    let profile_id = Uuid::from_u128(9);
    let schema = CatalogId::new(profile_id, CatalogKind::Schema, ["SERVICE", "APP"]);
    let request = lazydb::db::catalog_mutation::CatalogMutationRequest {
        connection: lazydb::identity::ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        request_id: 3,
        catalog_epoch: 1,
        mode: CatalogMutationMode::Create,
        anchor: CatalogMutationAnchor::Group {
            schema,
            group: lazydb::db::catalog::ObjectGroup::Tables,
        },
        object_type: CatalogObjectType::Catalog(CatalogKind::Table),
        current_database: Some("SERVICE".into()),
    };
    let mut table = TableDraft::new_for_database("APP", DatabaseKind::Oracle);
    table.name.set("invalid_table");
    table.columns[0].name.set("description");
    table.columns[0].native_type.set("TEXT");

    let error = OracleAdapter::plan_catalog_mutation(request, CatalogDraft::Table(table), None)
        .expect_err("Oracle must reject TEXT before execution");
    let message = error.to_string();
    assert!(message.contains("description"));
    assert!(message.contains("VARCHAR2"));
    assert!(message.contains("CLOB"));
}
