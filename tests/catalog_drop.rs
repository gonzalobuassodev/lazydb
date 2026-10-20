use lazydb::{
    action::{Action, Command},
    db::{
        catalog::{CatalogEntry, CatalogId, CatalogKind, OptionalMetadata, QualifiedName},
        catalog_drop::{
            CatalogDropError, CatalogDropExecutionTarget, CatalogDropPlan, CatalogDropRequest,
        },
        mssql::MsSqlAdapter,
        mysql::MySqlAdapter,
        oracle::OracleAdapter,
        postgres::PostgresAdapter,
        sqlite::SqliteAdapter,
    },
    identity::ConnectionIdentity,
};
use uuid::Uuid;

fn entry(profile_id: Uuid, kind: CatalogKind) -> CatalogEntry {
    let id = CatalogId::new(profile_id, kind, ["app", "public", "users"]);
    CatalogEntry {
        id,
        parent_id: None,
        kind,
        native_kind: "table".into(),
        qualified_name: QualifiedName {
            database: Some("app".into()),
            schema: Some("public".into()),
            object: "users".into(),
        },
        comment: OptionalMetadata::Unsupported,
        metadata: Default::default(),
        expandable: false,
        relation_id: None,
    }
}

fn oracle_entry(profile_id: Uuid, kind: CatalogKind, object: &str) -> CatalogEntry {
    let id = CatalogId::new(profile_id, kind, ["SUPPORTDB", "MFGSUPPORT", object]);
    CatalogEntry {
        id,
        parent_id: Some(CatalogId::new(
            profile_id,
            CatalogKind::Schema,
            ["SUPPORTDB", "MFGSUPPORT"],
        )),
        kind,
        native_kind: format!("{kind:?}"),
        qualified_name: QualifiedName {
            database: Some("SUPPORTDB".into()),
            schema: Some("MFGSUPPORT".into()),
            object: object.into(),
        },
        comment: OptionalMetadata::Unsupported,
        metadata: Default::default(),
        expandable: false,
        relation_id: None,
    }
}

#[test]
fn oracle_drop_plans_supported_objects_with_quoted_names() {
    let profile_id = Uuid::new_v4();
    let connection = ConnectionIdentity {
        profile_id,
        generation: 1,
    };
    for (kind, expected) in [
        (CatalogKind::Table, "DROP TABLE \"MFGSUPPORT\".\"tt1\""),
        (CatalogKind::View, "DROP VIEW \"MFGSUPPORT\".\"tt1\""),
        (
            CatalogKind::Sequence,
            "DROP SEQUENCE \"MFGSUPPORT\".\"tt1\"",
        ),
    ] {
        let object = oracle_entry(profile_id, kind, "tt1");
        let plan = OracleAdapter::plan_catalog_drop(
            CatalogDropRequest::new(connection, object.id.clone(), 1),
            &object,
        )
        .unwrap();
        assert_eq!(plan.sql(), expected);
    }
}

#[test]
fn oracle_drop_rejects_invalid_metadata_and_unsupported_kinds() {
    let profile_id = Uuid::new_v4();
    let connection = ConnectionIdentity {
        profile_id,
        generation: 1,
    };
    let mut invalid = oracle_entry(profile_id, CatalogKind::Table, "tt1");
    invalid.qualified_name.schema = Some("OTHER_SCHEMA".into());
    assert!(matches!(
        OracleAdapter::plan_catalog_drop(
            CatalogDropRequest::new(connection, invalid.id.clone(), 1),
            &invalid,
        ),
        Err(CatalogDropError::InvalidMetadata { .. })
    ));
    let index = oracle_entry(profile_id, CatalogKind::Index, "tt1");
    assert!(matches!(
        OracleAdapter::plan_catalog_drop(
            CatalogDropRequest::new(connection, index.id.clone(), 2),
            &index,
        ),
        Err(CatalogDropError::Unsupported { .. })
    ));
}

#[test]
fn plan_binds_request_identity_and_object() {
    let profile_id = Uuid::new_v4();
    let object = entry(profile_id, CatalogKind::Table);
    let request = CatalogDropRequest::new(
        ConnectionIdentity {
            profile_id,
            generation: 4,
        },
        object.id.clone(),
        9,
    );
    let plan =
        CatalogDropPlan::new(request.clone(), &object, "DROP TABLE \"app\".\"users\"").unwrap();

    assert_eq!(plan.request, request);
    assert_eq!(plan.object, object.id);
    assert_eq!(plan.kind, CatalogKind::Table);
    assert_eq!(plan.sql(), "DROP TABLE \"app\".\"users\"");
    assert!(plan.validate().is_ok());
}

#[test]
fn request_rejects_wrong_profile_empty_id_and_non_drop_kinds() {
    let profile_id = Uuid::new_v4();
    let wrong_profile = CatalogDropRequest::new(
        ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        CatalogId::new(
            Uuid::new_v4(),
            CatalogKind::Table,
            ["app", "public", "users"],
        ),
        1,
    );
    assert!(matches!(
        wrong_profile.validate(),
        Err(CatalogDropError::ProfileMismatch { .. })
    ));

    let empty = CatalogDropRequest::new(
        ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        CatalogId::new(profile_id, CatalogKind::Table, std::iter::empty::<String>()),
        1,
    );
    assert_eq!(empty.validate(), Err(CatalogDropError::EmptyObjectId));

    let database = CatalogDropRequest::new(
        ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        CatalogId::new(profile_id, CatalogKind::Database, ["app"]),
        1,
    );
    assert!(database.validate().is_ok());
}

#[test]
fn plan_rejects_forged_entry_and_round_trips_action_command_payloads() {
    let profile_id = Uuid::new_v4();
    let object = entry(profile_id, CatalogKind::Table);
    let request = CatalogDropRequest::new(
        ConnectionIdentity {
            profile_id,
            generation: 2,
        },
        object.id.clone(),
        3,
    );
    let mut forged = object.clone();
    forged.kind = CatalogKind::View;
    assert!(matches!(
        CatalogDropPlan::new(request.clone(), &forged, "DROP VIEW \"app\".\"users\""),
        Err(CatalogDropError::KindMismatch { .. })
    ));

    let action = Action::CatalogDropPlanFailed {
        request: request.clone(),
        error: CatalogDropError::EmptyObjectName,
    };
    assert!(
        matches!(action, Action::CatalogDropPlanFailed { request: found, .. } if found == request)
    );
    let command = Command::PlanCatalogDrop(request.clone());
    assert!(matches!(command, Command::PlanCatalogDrop(found) if found == request));
}

#[test]
fn plan_rejects_unsafe_sql_but_allows_quoted_identifier_text() {
    let profile_id = Uuid::new_v4();
    let object = entry(profile_id, CatalogKind::Table);
    let request = CatalogDropRequest::new(
        ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        object.id.clone(),
        1,
    );

    assert!(CatalogDropPlan::new(request.clone(), &object, "DROP TABLE \"cascade\"").is_ok());
    assert!(matches!(
        CatalogDropPlan::new(request.clone(), &object, "DROP TABLE users CASCADE"),
        Err(CatalogDropError::UnsafeSql)
    ));
    assert!(matches!(
        CatalogDropPlan::new(request.clone(), &object, "DROP TABLE IF EXISTS users"),
        Err(CatalogDropError::UnsafeSql)
    ));
    assert!(matches!(
        CatalogDropPlan::new(request, &object, "DROP TABLE users; SELECT 1"),
        Err(CatalogDropError::UnsafeSql)
    ));
}

#[test]
fn postgres_planner_quotes_supported_objects_and_relation_children() {
    let profile_id = Uuid::new_v4();
    let connection = ConnectionIdentity {
        profile_id,
        generation: 8,
    };
    let table = CatalogEntry {
        id: CatalogId::new(
            profile_id,
            CatalogKind::Table,
            ["db", "odd\"schema", "odd\"table", "1"],
        ),
        parent_id: None,
        kind: CatalogKind::Table,
        native_kind: "table".into(),
        qualified_name: QualifiedName {
            database: Some("db".into()),
            schema: Some("odd\"schema".into()),
            object: "odd\"table".into(),
        },
        comment: OptionalMetadata::Unsupported,
        metadata: Default::default(),
        expandable: true,
        relation_id: None,
    };
    let request = CatalogDropRequest::new(connection, table.id.clone(), 1);
    assert_eq!(
        PostgresAdapter::plan_catalog_drop(request, &table)
            .unwrap()
            .sql(),
        "DROP TABLE \"odd\"\"schema\".\"odd\"\"table\""
    );

    let column = CatalogEntry {
        id: CatalogId::new(
            profile_id,
            CatalogKind::Column,
            ["db", "odd\"schema", "odd\"table", "1", "2"],
        ),
        parent_id: None,
        kind: CatalogKind::Column,
        native_kind: "column".into(),
        qualified_name: QualifiedName {
            database: Some("db".into()),
            schema: Some("odd\"schema".into()),
            object: "col\"umn".into(),
        },
        comment: OptionalMetadata::Unsupported,
        metadata: Default::default(),
        expandable: false,
        relation_id: Some(table.id.clone()),
    };
    let request = CatalogDropRequest::new(connection, column.id.clone(), 2);
    assert_eq!(
        PostgresAdapter::plan_catalog_drop(request, &column)
            .unwrap()
            .sql(),
        "ALTER TABLE \"odd\"\"schema\".\"odd\"\"table\" DROP COLUMN \"col\"\"umn\""
    );

    for (kind, prefix) in [
        (CatalogKind::View, "DROP VIEW"),
        (CatalogKind::MaterializedView, "DROP MATERIALIZED VIEW"),
        (CatalogKind::Sequence, "DROP SEQUENCE"),
        (CatalogKind::Type, "DROP TYPE"),
        (CatalogKind::Index, "DROP INDEX"),
    ] {
        let mut object = table.clone();
        object.id.kind = kind;
        object.kind = kind;
        let request = CatalogDropRequest::new(connection, object.id.clone(), 3);
        assert_eq!(
            PostgresAdapter::plan_catalog_drop(request, &object)
                .unwrap()
                .sql(),
            format!("{prefix} \"odd\"\"schema\".\"odd\"\"table\"")
        );
    }

    for (kind, name) in [
        (CatalogKind::PrimaryKey, "pk"),
        (CatalogKind::UniqueConstraint, "unique"),
        (CatalogKind::ForeignKey, "fk"),
        (CatalogKind::CheckConstraint, "check"),
    ] {
        let mut constraint = column.clone();
        constraint.id.kind = kind;
        constraint.kind = kind;
        constraint.qualified_name.object = name.into();
        let request = CatalogDropRequest::new(connection, constraint.id.clone(), 4);
        assert_eq!(
            PostgresAdapter::plan_catalog_drop(request, &constraint)
                .unwrap()
                .sql(),
            format!("ALTER TABLE \"odd\"\"schema\".\"odd\"\"table\" DROP CONSTRAINT \"{name}\"")
        );
    }

    let mut trigger = column.clone();
    trigger.id.kind = CatalogKind::Trigger;
    trigger.kind = CatalogKind::Trigger;
    trigger.qualified_name.object = "audit\"trigger".into();
    let request = CatalogDropRequest::new(connection, trigger.id.clone(), 5);
    assert_eq!(
        PostgresAdapter::plan_catalog_drop(request, &trigger)
            .unwrap()
            .sql(),
        "DROP TRIGGER \"audit\"\"trigger\" ON \"odd\"\"schema\".\"odd\"\"table\""
    );
}

#[test]
fn postgres_schema_planner_quotes_name_and_requires_restrict() {
    let profile_id = Uuid::new_v4();
    let connection = ConnectionIdentity {
        profile_id,
        generation: 8,
    };
    let schema = CatalogEntry {
        id: CatalogId::new(profile_id, CatalogKind::Schema, ["db", "odd\"schema"]),
        parent_id: Some(CatalogId::new(profile_id, CatalogKind::Database, ["db"])),
        kind: CatalogKind::Schema,
        native_kind: "schema".into(),
        qualified_name: QualifiedName {
            database: Some("db".into()),
            schema: Some("odd\"schema".into()),
            object: "odd\"schema".into(),
        },
        comment: OptionalMetadata::Unsupported,
        metadata: Default::default(),
        expandable: true,
        relation_id: None,
    };
    let request = CatalogDropRequest::new(connection, schema.id.clone(), 6);

    assert_eq!(
        PostgresAdapter::plan_catalog_drop(request, &schema)
            .unwrap()
            .sql(),
        "DROP SCHEMA \"odd\"\"schema\" RESTRICT"
    );
}

#[test]
fn postgres_schema_plan_can_be_explicitly_upgraded_to_cascade() {
    let profile_id = Uuid::new_v4();
    let connection = ConnectionIdentity {
        profile_id,
        generation: 8,
    };
    let schema = CatalogEntry {
        id: CatalogId::new(profile_id, CatalogKind::Schema, ["db", "odd\"schema"]),
        parent_id: Some(CatalogId::new(profile_id, CatalogKind::Database, ["db"])),
        kind: CatalogKind::Schema,
        native_kind: "schema".into(),
        qualified_name: QualifiedName {
            database: Some("db".into()),
            schema: Some("odd\"schema".into()),
            object: "odd\"schema".into(),
        },
        comment: OptionalMetadata::Unsupported,
        metadata: Default::default(),
        expandable: true,
        relation_id: None,
    };
    let mut request =
        CatalogDropRequest::new(connection, schema.id.clone(), 6).with_entry(schema.clone());
    request.catalog_epoch = 12;
    let restrict = PostgresAdapter::plan_catalog_drop(request.clone(), &schema).unwrap();
    assert_eq!(restrict.sql(), "DROP SCHEMA \"odd\"\"schema\" RESTRICT");

    let cascade = PostgresAdapter::plan_catalog_drop_cascade(request, &schema).unwrap();
    assert_eq!(cascade.sql(), "DROP SCHEMA \"odd\"\"schema\" CASCADE");
    assert_eq!(cascade.request.connection, connection);
    assert_eq!(cascade.request.catalog_epoch, 12);
    assert_eq!(cascade.object, schema.id);
    assert_eq!(cascade.kind, CatalogKind::Schema);
    let rebound = cascade.clone().with_request_id(99).unwrap();
    assert_eq!(rebound.request.request_id, 99);
    assert_eq!(rebound.request.connection, connection);
    assert_eq!(rebound.request.catalog_epoch, 12);
    assert_eq!(rebound.sql(), cascade.sql());

    let mut forged = cascade.clone();
    forged.qualified_name = "different_schema".into();
    assert_eq!(forged.validate(), Err(CatalogDropError::UnsafeSql));
    let mut forged_entry = cascade.clone();
    forged_entry
        .request
        .entry
        .as_mut()
        .unwrap()
        .qualified_name
        .schema = Some("different_schema".into());
    assert_eq!(forged_entry.validate(), Err(CatalogDropError::UnsafeSql));
}

#[test]
fn postgres_schema_planner_rejects_incomplete_namespace_metadata() {
    let profile_id = Uuid::new_v4();
    let connection = ConnectionIdentity {
        profile_id,
        generation: 8,
    };
    let mut schema = entry(profile_id, CatalogKind::Schema);
    schema.id.native_path = vec!["db".into(), "public".into()];
    schema.parent_id = None;
    schema.qualified_name.database = None;
    schema.qualified_name.schema = None;
    schema.qualified_name.object = "public".into();
    let request = CatalogDropRequest::new(connection, schema.id.clone(), 7);

    assert!(matches!(
        PostgresAdapter::plan_catalog_drop(request, &schema),
        Err(CatalogDropError::InvalidMetadata {
            kind: CatalogKind::Schema,
            ..
        })
    ));
}

#[test]
fn postgres_database_planner_requires_a_different_maintenance_database() {
    let profile_id = Uuid::new_v4();
    let connection = ConnectionIdentity {
        profile_id,
        generation: 8,
    };
    let database = entry(profile_id, CatalogKind::Database);
    let request = CatalogDropRequest::new(connection, database.id.clone(), 8);
    assert!(matches!(
        PostgresAdapter::plan_catalog_drop(request, &database),
        Err(CatalogDropError::Unsupported {
            kind: CatalogKind::Database,
            ..
        })
    ));

    let request = CatalogDropRequest::new(connection, database.id.clone(), 9)
        .with_maintenance_database("postgres");
    let plan = PostgresAdapter::plan_catalog_drop(request, &database).unwrap();
    assert_eq!(plan.sql(), "DROP DATABASE \"users\"");
    assert_eq!(
        plan.execution_target,
        CatalogDropExecutionTarget::MaintenanceDatabase("postgres".into())
    );
}

#[test]
fn postgres_planner_reports_unsupported_and_insufficient_child_metadata() {
    let profile_id = Uuid::new_v4();
    let connection = ConnectionIdentity {
        profile_id,
        generation: 1,
    };
    let database = entry(profile_id, CatalogKind::Database);
    let request = CatalogDropRequest::new(connection, database.id.clone(), 1);
    assert!(matches!(
        PostgresAdapter::plan_catalog_drop(request, &database),
        Err(CatalogDropError::Unsupported {
            kind: CatalogKind::Database,
            ..
        })
    ));

    let column = entry(profile_id, CatalogKind::Column);
    let request = CatalogDropRequest::new(connection, column.id.clone(), 2);
    assert!(matches!(
        PostgresAdapter::plan_catalog_drop(request, &column),
        Err(CatalogDropError::Unsupported {
            kind: CatalogKind::Column,
            ..
        })
    ));
}

#[test]
fn catalog_drop_errors_distinguish_unsupported_kinds_from_invalid_metadata() {
    let unsupported = CatalogDropError::Unsupported {
        kind: CatalogKind::Database,
        reason: "not implemented".into(),
    };
    assert_eq!(
        unsupported.to_string(),
        "catalog drop for Database is unsupported: not implemented"
    );

    let invalid = CatalogDropError::InvalidMetadata {
        kind: CatalogKind::Schema,
        reason: "database name is missing".into(),
    };
    assert_eq!(
        invalid.to_string(),
        "catalog drop metadata is incomplete for Schema: database name is missing"
    );
}

#[test]
fn mysql_planner_generates_safe_supported_drop_statements() {
    let profile_id = Uuid::new_v4();
    let connection = ConnectionIdentity {
        profile_id,
        generation: 3,
    };
    let relation = CatalogId::new(profile_id, CatalogKind::Table, ["db", "db", "users"]);
    let schema = CatalogId::new(profile_id, CatalogKind::Schema, ["db", "db"]);
    let make_entry = |kind: CatalogKind,
                      id: CatalogId,
                      object: &str,
                      relation_id: Option<CatalogId>| CatalogEntry {
        id,
        parent_id: Some(schema.clone()),
        kind,
        native_kind: "mysql".into(),
        qualified_name: QualifiedName {
            database: Some("db".into()),
            schema: Some("db".into()),
            object: object.to_owned(),
        },
        comment: OptionalMetadata::Unsupported,
        metadata: Default::default(),
        expandable: false,
        relation_id,
    };

    let cases = [
        (
            make_entry(
                CatalogKind::Table,
                relation.clone(),
                "users",
                Some(relation.clone()),
            ),
            "DROP TABLE `db`.`users`",
        ),
        (
            make_entry(
                CatalogKind::View,
                CatalogId::new(profile_id, CatalogKind::View, ["db", "db", "report`view"]),
                "report`view",
                None,
            ),
            "DROP VIEW `db`.`report``view`",
        ),
        (
            make_entry(
                CatalogKind::Index,
                CatalogId::new(
                    profile_id,
                    CatalogKind::Index,
                    ["db", "db", "users", "ix`x"],
                ),
                "ix`x",
                Some(relation.clone()),
            ),
            "ALTER TABLE `db`.`users` DROP INDEX `ix``x`",
        ),
        (
            make_entry(
                CatalogKind::PrimaryKey,
                CatalogId::new(
                    profile_id,
                    CatalogKind::PrimaryKey,
                    ["db", "db", "users", "PRIMARY"],
                ),
                "PRIMARY",
                Some(relation.clone()),
            ),
            "ALTER TABLE `db`.`users` DROP PRIMARY KEY",
        ),
        (
            make_entry(
                CatalogKind::ForeignKey,
                CatalogId::new(
                    profile_id,
                    CatalogKind::ForeignKey,
                    ["db", "db", "users", "fk_users"],
                ),
                "fk_users",
                Some(relation.clone()),
            ),
            "ALTER TABLE `db`.`users` DROP FOREIGN KEY `fk_users`",
        ),
        (
            make_entry(
                CatalogKind::UniqueConstraint,
                CatalogId::new(
                    profile_id,
                    CatalogKind::UniqueConstraint,
                    ["db", "db", "users", "uq_users"],
                ),
                "uq_users",
                Some(relation.clone()),
            ),
            "ALTER TABLE `db`.`users` DROP INDEX `uq_users`",
        ),
        (
            make_entry(
                CatalogKind::Trigger,
                CatalogId::new(profile_id, CatalogKind::Trigger, ["db", "db", "audit`tr"]),
                "audit`tr",
                Some(relation.clone()),
            ),
            "DROP TRIGGER `db`.`audit``tr`",
        ),
    ];
    for (entry, expected) in cases {
        let request = CatalogDropRequest::new(connection, entry.id.clone(), 1);
        assert_eq!(
            MySqlAdapter::plan_catalog_drop(request, &entry)
                .unwrap()
                .sql(),
            expected
        );
    }

    for (kind, expected) in [
        (CatalogKind::Database, "DROP DATABASE `db`"),
        (CatalogKind::Schema, "DROP DATABASE `db`"),
    ] {
        let id = if kind == CatalogKind::Database {
            CatalogId::new(profile_id, kind, ["db"])
        } else {
            schema.clone()
        };
        let entry = CatalogEntry {
            id: id.clone(),
            parent_id: None,
            kind,
            native_kind: "mysql".into(),
            qualified_name: QualifiedName {
                database: Some("db".into()),
                schema: (kind == CatalogKind::Schema).then(|| "db".into()),
                object: "db".into(),
            },
            comment: OptionalMetadata::Unsupported,
            metadata: Default::default(),
            expandable: false,
            relation_id: None,
        };
        let request = CatalogDropRequest::new(connection, id, 2);
        assert_eq!(
            MySqlAdapter::plan_catalog_drop(request, &entry)
                .unwrap()
                .sql(),
            expected
        );
    }

    for kind in [CatalogKind::Function, CatalogKind::Procedure] {
        let id = CatalogId::new(profile_id, kind, ["db", "db", "routine", "specific"]);
        let entry = make_entry(kind, id.clone(), "routine", None);
        let request = CatalogDropRequest::new(connection, id, 3);
        let keyword = if kind == CatalogKind::Function {
            "FUNCTION"
        } else {
            "PROCEDURE"
        };
        assert_eq!(
            MySqlAdapter::plan_catalog_drop(request, &entry)
                .unwrap()
                .sql(),
            format!("DROP {keyword} `db`.`routine`")
        );
    }
}

#[test]
fn mysql_planner_reports_unsupported_or_insufficient_metadata() {
    let profile_id = Uuid::new_v4();
    let connection = ConnectionIdentity {
        profile_id,
        generation: 1,
    };
    for kind in [
        CatalogKind::MaterializedView,
        CatalogKind::Type,
        CatalogKind::Column,
        CatalogKind::CheckConstraint,
    ] {
        let object = entry(profile_id, kind);
        let request = CatalogDropRequest::new(connection, object.id.clone(), 1);
        assert!(matches!(
            MySqlAdapter::plan_catalog_drop(request, &object),
            Err(CatalogDropError::Unsupported { kind: found, .. }) if found == kind
        ));
    }

    let mut index = entry(profile_id, CatalogKind::Index);
    index.id.native_path = vec!["app", "public", "users", "ix"]
        .into_iter()
        .map(String::from)
        .collect();
    let request = CatalogDropRequest::new(connection, index.id.clone(), 2);
    assert!(matches!(
        MySqlAdapter::plan_catalog_drop(request, &index),
        Err(CatalogDropError::Unsupported {
            kind: CatalogKind::Index,
            ..
        })
    ));
}

#[test]
fn sqlite_planner_quotes_attached_schema_objects_without_unsafe_clauses() {
    let profile_id = Uuid::new_v4();
    let connection = ConnectionIdentity {
        profile_id,
        generation: 3,
    };
    for (kind, prefix) in [
        (CatalogKind::Table, "DROP TABLE"),
        (CatalogKind::View, "DROP VIEW"),
        (CatalogKind::Index, "DROP INDEX"),
        (CatalogKind::Trigger, "DROP TRIGGER"),
    ] {
        let entry = CatalogEntry {
            id: CatalogId::new(
                profile_id,
                kind,
                ["catalog.db", "Archive\"Case", "odd\"object"],
            ),
            parent_id: None,
            kind,
            native_kind: "sqlite_object".into(),
            qualified_name: QualifiedName {
                database: Some("catalog.db".into()),
                schema: Some("Archive\"Case".into()),
                object: "odd\"object".into(),
            },
            comment: OptionalMetadata::Unsupported,
            metadata: Default::default(),
            expandable: false,
            relation_id: None,
        };
        let request = CatalogDropRequest::new(connection, entry.id.clone(), 1);
        let plan = SqliteAdapter::plan_catalog_drop(request, &entry).unwrap();
        assert_eq!(
            plan.sql(),
            format!("{prefix} \"Archive\"\"Case\".\"odd\"\"object\"")
        );
        assert!(!plan.sql().contains("CASCADE"));
        assert!(!plan.sql().contains("IF EXISTS"));
    }
}

#[test]
fn sqlite_planner_rejects_objects_requiring_unsupported_or_rebuild_operations() {
    let profile_id = Uuid::new_v4();
    let connection = ConnectionIdentity {
        profile_id,
        generation: 4,
    };
    for kind in [
        CatalogKind::Database,
        CatalogKind::Schema,
        CatalogKind::MaterializedView,
        CatalogKind::Function,
        CatalogKind::Procedure,
        CatalogKind::Sequence,
        CatalogKind::Type,
        CatalogKind::Column,
        CatalogKind::PrimaryKey,
        CatalogKind::UniqueConstraint,
        CatalogKind::ForeignKey,
        CatalogKind::CheckConstraint,
    ] {
        let entry = entry(profile_id, kind);
        let request = CatalogDropRequest::new(connection, entry.id.clone(), 2);
        assert!(matches!(
            SqliteAdapter::plan_catalog_drop(request, &entry),
            Err(CatalogDropError::Unsupported { kind: found, .. }) if found == kind
        ));
    }
}

#[test]
fn sql_server_planner_generates_supported_drops_and_rejects_children_safely() {
    let profile_id = Uuid::new_v4();
    let connection = ConnectionIdentity {
        profile_id,
        generation: 1,
    };
    let schema = CatalogId::new(profile_id, CatalogKind::Schema, ["db", "schema"]);
    let relation = CatalogId::new(
        profile_id,
        CatalogKind::Table,
        ["db", "schema", "table", "42"],
    );
    let make = |kind: CatalogKind,
                id: CatalogId,
                parent_id: Option<CatalogId>,
                relation_id: Option<CatalogId>,
                object: &str| CatalogEntry {
        id,
        parent_id,
        kind,
        native_kind: "sql_server".into(),
        qualified_name: QualifiedName {
            database: Some("db".into()),
            schema: Some("schema".into()),
            object: object.to_owned(),
        },
        comment: OptionalMetadata::Unsupported,
        metadata: Default::default(),
        expandable: false,
        relation_id,
    };
    for (kind, keyword) in [
        (CatalogKind::Table, "TABLE"),
        (CatalogKind::View, "VIEW"),
        (CatalogKind::Sequence, "SEQUENCE"),
        (CatalogKind::Function, "FUNCTION"),
        (CatalogKind::Procedure, "PROCEDURE"),
    ] {
        let id = if kind == CatalogKind::Table {
            relation.clone()
        } else {
            CatalogId::new(profile_id, kind, ["db", "schema", "object", "42"])
        };
        let object_name = if kind == CatalogKind::Table {
            "table"
        } else {
            "object"
        };
        let object = make(kind, id.clone(), Some(schema.clone()), None, object_name);
        let request = CatalogDropRequest::new(connection, id, 1);
        assert_eq!(
            MsSqlAdapter::plan_catalog_drop(request, &object)
                .unwrap()
                .sql(),
            format!("DROP {keyword} [db].[schema].[{object_name}]")
        );
    }

    let index_id = CatalogId::new(
        profile_id,
        CatalogKind::Index,
        ["db", "schema", "table", "42", "7"],
    );
    let index = make(
        CatalogKind::Index,
        index_id.clone(),
        Some(relation.clone()),
        Some(relation.clone()),
        "ix]name",
    );
    assert_eq!(
        MsSqlAdapter::plan_catalog_drop(CatalogDropRequest::new(connection, index_id, 2), &index)
            .unwrap()
            .sql(),
        "DROP INDEX [ix]]name] ON [db].[schema].[table]"
    );

    let trigger_id = CatalogId::new(
        profile_id,
        CatalogKind::Trigger,
        ["db", "schema", "audit", "99"],
    );
    let trigger = make(
        CatalogKind::Trigger,
        trigger_id.clone(),
        Some(relation.clone()),
        Some(relation.clone()),
        "audit",
    );
    assert_eq!(
        MsSqlAdapter::plan_catalog_drop(
            CatalogDropRequest::new(connection, trigger_id, 2),
            &trigger,
        )
        .unwrap()
        .sql(),
        "DROP TRIGGER [db].[schema].[audit]"
    );
    let mut forged_index = index.clone();
    forged_index.parent_id = Some(CatalogId::new(
        profile_id,
        CatalogKind::Table,
        ["db", "schema", "other", "42"],
    ));
    assert!(matches!(
        MsSqlAdapter::plan_catalog_drop(
            CatalogDropRequest::new(connection, forged_index.id.clone(), 2),
            &forged_index,
        ),
        Err(CatalogDropError::Unsupported {
            kind: CatalogKind::Index,
            reason,
        }) if reason == "catalog entry has an invalid owning relation identity"
    ));

    let odd_id = CatalogId::new(
        profile_id,
        CatalogKind::Table,
        ["db.;\n", "schema].", "table;\r", "43"],
    );
    let odd = CatalogEntry {
        id: odd_id.clone(),
        parent_id: Some(CatalogId::new(
            profile_id,
            CatalogKind::Schema,
            ["db.;\n", "schema]."],
        )),
        kind: CatalogKind::Table,
        native_kind: "sql_server".into(),
        qualified_name: QualifiedName {
            database: Some("db.;\n".into()),
            schema: Some("schema].".into()),
            object: "table;\r".into(),
        },
        comment: OptionalMetadata::Unsupported,
        metadata: Default::default(),
        expandable: false,
        relation_id: None,
    };
    let odd_sql =
        MsSqlAdapter::plan_catalog_drop(CatalogDropRequest::new(connection, odd_id, 4), &odd)
            .unwrap()
            .sql()
            .to_owned();
    assert_eq!(odd_sql, "DROP TABLE [db.;\n].[schema]].].[table;\r]");
    assert!(
        CatalogDropPlan::new(
            CatalogDropRequest::new(connection, odd.id.clone(), 4),
            &odd,
            odd_sql,
        )
        .is_ok()
    );

    for kind in [
        CatalogKind::Column,
        CatalogKind::PrimaryKey,
        CatalogKind::UniqueConstraint,
        CatalogKind::ForeignKey,
        CatalogKind::CheckConstraint,
    ] {
        let id = CatalogId::new(profile_id, kind, ["db", "schema", "table", "42", "7"]);
        let object = make(
            kind,
            id.clone(),
            Some(relation.clone()),
            Some(relation.clone()),
            "child",
        );
        assert!(matches!(
            MsSqlAdapter::plan_catalog_drop(CatalogDropRequest::new(connection, id, 3), &object),
            Err(CatalogDropError::Unsupported { kind: found, .. }) if found == kind
        ));
    }
}
