use lazydb::model::catalog_editor::{CatalogDraft, SchemaDraft};
use lazydb::{
    db::{
        DatabaseConnection, ErrorCategory,
        catalog::{
            CatalogId, CatalogKind, CatalogRequest, CatalogRequestKey, CatalogSearchObjectScope,
            CatalogSearchRequest, CatalogTarget,
        },
        catalog_mutation::{
            CatalogMutationAnchor, CatalogMutationMode, CatalogMutationRequest,
            CatalogObjectDefinitionRequest, CatalogObjectType,
        },
    },
    identity::ConnectionIdentity,
    model::execution_target::ExecutionTarget,
    profile::import_connection_url,
};
use tempfile::TempDir;

#[tokio::test]
async fn duckdb_executes_queries_and_reopens_a_file_database() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("analytics.duckdb");
    let url = format!("duckdb://{}", path.display());
    let profile = import_connection_url(&url, Some("analytics"))
        .unwrap()
        .profile;

    let first = DatabaseConnection::connect(&profile, None).await.unwrap();
    let created = first
        .execute("CREATE TABLE events (id INTEGER, label VARCHAR); INSERT INTO events VALUES (7, 'opened')")
        .await
        .unwrap();
    assert_eq!(created.result_sets.len(), 2);
    let returned = first
        .execute("INSERT INTO events VALUES (8, 'returned') RETURNING id, label")
        .await
        .unwrap();
    assert_eq!(returned.result_sets[0].rows.len(), 1);
    first.close().await;

    let reopened = DatabaseConnection::connect(&profile, None).await.unwrap();
    let result = reopened
        .execute("SELECT id, label FROM events")
        .await
        .unwrap();
    assert_eq!(result.result_sets.len(), 1);
    assert_eq!(result.result_sets[0].columns.len(), 2);
    assert_eq!(result.result_sets[0].rows.len(), 2);
    assert_eq!(
        result.result_sets[0].rows[0],
        vec![
            lazydb::db::value::CellValue::Integer(7),
            lazydb::db::value::CellValue::Text("opened".into()),
        ]
    );
    assert_eq!(
        result.result_sets[0].rows[1][0],
        lazydb::db::value::CellValue::Integer(8)
    );
    assert_eq!(
        result.result_sets[0].columns[1]
            .type_name
            .to_ascii_uppercase(),
        "UTF8"
    );
    reopened.close().await;
}

#[tokio::test]
async fn duckdb_executes_queries_in_memory() {
    let profile = import_connection_url("duckdb::memory:", Some("memory"))
        .unwrap()
        .profile;
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    let result = database.execute("SELECT 42 AS answer").await.unwrap();
    assert_eq!(
        result.result_sets[0].rows[0][0],
        lazydb::db::value::CellValue::Integer(42)
    );
    database.close().await;
}

#[tokio::test]
async fn duckdb_preserves_statement_boundaries_and_empty_result_metadata() {
    let profile = import_connection_url("duckdb::memory:", Some("batch"))
        .unwrap()
        .profile;
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    let result = database
        .execute("SELECT 'a;b' AS first; SELECT 2 AS second WHERE false; SELECT 3 AS third")
        .await
        .unwrap();
    assert_eq!(result.result_sets.len(), 3);
    assert_eq!(
        result.result_sets[0].rows[0][0],
        lazydb::db::value::CellValue::Text("a;b".into())
    );
    assert_eq!(result.result_sets[1].columns[0].name, "second");
    assert!(result.result_sets[1].rows.is_empty());
    assert_eq!(
        result.result_sets[2].rows[0][0],
        lazydb::db::value::CellValue::Integer(3)
    );

    let error = database
        .execute("SELECT 1 AS before; SELECT * FROM missing_table; SELECT 3 AS after")
        .await
        .unwrap_err();
    assert_eq!(error.category, ErrorCategory::Sql);
    let next = database.execute("SELECT 4 AS still_usable").await.unwrap();
    assert_eq!(
        next.result_sets[0].rows[0][0],
        lazydb::db::value::CellValue::Integer(4)
    );
    database.close().await;
}

#[tokio::test]
async fn duckdb_decodes_scalar_values_without_precision_loss() {
    use lazydb::db::value::CellValue;
    let profile = import_connection_url("duckdb::memory:", Some("values"))
        .unwrap()
        .profile;
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    let result = database
        .execute("SELECT 18446744073709551615::UBIGINT AS u, '12345678901234567890123456789.123456789'::DECIMAL(38,9) AS d, DATE '2026-10-15' AS day, TIMESTAMP_NS '2026-10-15 12:34:56.123456789' AS ts, TIME '12:34:56.123456' AS clock, INTERVAL '2 days 3 seconds' AS span")
        .await
        .unwrap();
    let row = &result.result_sets[0].rows[0];
    assert_eq!(row[0], CellValue::Unsigned(u64::MAX));
    assert_eq!(
        row[1],
        CellValue::Text("12345678901234567890123456789.123456789".into())
    );
    assert_eq!(
        row[2],
        CellValue::Date(chrono::NaiveDate::from_ymd_opt(2026, 10, 15).unwrap())
    );
    assert_eq!(
        row[3],
        CellValue::Timestamp(
            chrono::DateTime::parse_from_rfc3339("2026-10-15T12:34:56.123456789+00:00").unwrap()
        )
    );
    assert_eq!(
        row[4],
        CellValue::Time(chrono::NaiveTime::from_hms_micro_opt(12, 34, 56, 123_456).unwrap())
    );
    assert_eq!(
        row[5],
        CellValue::Text("0 mons 2 days 3000000000 ns".into())
    );
    database.close().await;
}

#[tokio::test]
async fn duckdb_time_ns_scalar_select_decodes_nanoseconds_and_nulls_without_panicking() {
    let profile = import_connection_url("duckdb::memory:", Some("time-ns"))
        .unwrap()
        .profile;
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    let result = database
        .execute("SELECT TIME_NS '12:34:56.123456789' AS \"clock face\", NULL::TIME_NS AS missing UNION ALL SELECT NULL::TIME_NS, NULL::TIME_NS")
        .await
        .unwrap();
    assert_eq!(result.result_sets[0].columns[0].type_name, "Time64(ns)");
    assert_eq!(result.result_sets[0].rows.len(), 2);
    assert_eq!(
        result.result_sets[0].rows[0][0],
        lazydb::db::value::CellValue::Time(
            chrono::NaiveTime::from_hms_nano_opt(12, 34, 56, 123_456_789).unwrap(),
        )
    );
    assert_eq!(
        result.result_sets[0].rows[0][1],
        lazydb::db::value::CellValue::Null
    );
    assert_eq!(
        result.result_sets[0].rows[1][0],
        lazydb::db::value::CellValue::Null
    );
    database.close().await;
}

#[tokio::test]
async fn duckdb_time_ns_from_first_query_decodes_nanoseconds_and_nulls() {
    let profile = import_connection_url("duckdb::memory:", Some("time-ns-from-first"))
        .unwrap()
        .profile;
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    let result = database
        .execute("FROM (VALUES (TIME_NS '12:34:56.123456789'), (NULL::TIME_NS)) AS t(clock) SELECT clock")
        .await
        .unwrap();
    assert_eq!(result.result_sets[0].columns[0].type_name, "Time64(ns)");
    assert_eq!(result.result_sets[0].rows.len(), 2);
    assert_eq!(
        result.result_sets[0].rows[0][0],
        lazydb::db::value::CellValue::Time(
            chrono::NaiveTime::from_hms_nano_opt(12, 34, 56, 123_456_789).unwrap(),
        )
    );
    assert_eq!(
        result.result_sets[0].rows[1][0],
        lazydb::db::value::CellValue::Null
    );
    database.close().await;
}

#[tokio::test]
async fn duckdb_time_ns_result_decodes_across_multiple_chunks() {
    let profile = import_connection_url("duckdb::memory:", Some("time-ns-chunks"))
        .unwrap()
        .profile;
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    let result = database
        .execute("SELECT CASE WHEN i = 2050 THEN NULL ELSE TIME_NS '23:59:58.987654321' END AS clock FROM range(2051) t(i)")
        .await
        .unwrap();
    let rows = &result.result_sets[0].rows;
    assert_eq!(rows.len(), 2051);
    assert_eq!(
        rows[2049][0],
        lazydb::db::value::CellValue::Time(
            chrono::NaiveTime::from_hms_nano_opt(23, 59, 58, 987_654_321).unwrap(),
        )
    );
    assert_eq!(rows[2050][0], lazydb::db::value::CellValue::Null);
    database.close().await;
}

#[tokio::test]
async fn duckdb_time_cast_with_query_preserves_engine_result_type() {
    let profile = import_connection_url("duckdb::memory:", Some("time-ns-with"))
        .unwrap()
        .profile;
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    let result = database.execute(
        "WITH values AS (SELECT TIME_NS '01:02:03.000000004'::TIME AS clock) SELECT clock FROM values",
    ).await.unwrap();
    assert_eq!(result.result_sets[0].columns[0].type_name, "Time64(µs)");
    assert_eq!(result.result_sets[0].rows.len(), 1);
    database.close().await;
}

#[tokio::test]
async fn duckdb_schema_create_and_table_view_create_drop_contracts() {
    let profile = import_connection_url("duckdb::memory:", Some("catalog-mutations"))
        .unwrap()
        .profile;
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    let identity = ConnectionIdentity {
        profile_id: profile.id,
        generation: 1,
    };
    let mut schema_draft = SchemaDraft::new();
    schema_draft.name = "sales space".into();
    let schema_request = CatalogMutationRequest {
        connection: identity,
        request_id: 1,
        catalog_epoch: 0,
        mode: CatalogMutationMode::Create,
        anchor: CatalogMutationAnchor::Catalog(CatalogId::new(
            profile.id,
            CatalogKind::Database,
            ["memory"],
        )),
        object_type: CatalogObjectType::Catalog(CatalogKind::Schema),
        current_database: Some("memory".into()),
    };
    let schema_plan = database
        .plan_catalog_mutation(schema_request, CatalogDraft::Schema(schema_draft), None)
        .unwrap();
    assert!(schema_plan.statements()[0].contains("\"sales space\""));
    database
        .execute_catalog_mutation(&schema_plan)
        .await
        .unwrap();

    let mut table_draft = lazydb::model::catalog_editor::TableDraft::new_for_database(
        "sales space",
        lazydb::profile::DatabaseKind::DuckDb,
    );
    table_draft.name = "order items".into();
    table_draft.schema = "sales space".into();
    table_draft.columns[0].name = "id".into();
    table_draft.columns[0].native_type = "INTEGER".into();
    table_draft.columns[0].nullable = false;
    let table_request = CatalogMutationRequest {
        connection: identity,
        request_id: 2,
        catalog_epoch: 0,
        mode: CatalogMutationMode::Create,
        anchor: CatalogMutationAnchor::Catalog(CatalogId::new(
            profile.id,
            CatalogKind::Schema,
            ["memory", "sales space"],
        )),
        object_type: CatalogObjectType::Catalog(CatalogKind::Table),
        current_database: Some("memory".into()),
    };
    let table_plan = database
        .plan_catalog_mutation(table_request, CatalogDraft::Table(table_draft), None)
        .unwrap();
    database
        .execute_catalog_mutation(&table_plan)
        .await
        .unwrap();
    let view_plan = database
        .plan_catalog_mutation(
            CatalogMutationRequest {
                connection: identity,
                request_id: 3,
                catalog_epoch: 0,
                mode: CatalogMutationMode::Create,
                anchor: CatalogMutationAnchor::Catalog(CatalogId::new(
                    profile.id,
                    CatalogKind::Schema,
                    ["memory", "sales space"],
                )),
                object_type: CatalogObjectType::Catalog(CatalogKind::View),
                current_database: Some("memory".into()),
            },
            CatalogDraft::View({
                let definition = lazydb::db::catalog_mutation::ViewDefinition {
                    database: "memory".into(),
                    schema: "sales space".into(),
                    name: "order view".into(),
                    owner: String::new(),
                    comment: Default::default(),
                    query: "SELECT id FROM \"sales space\".\"order items\"".into(),
                    output_columns: vec!["id".into()],
                    security_barrier: lazydb::db::catalog_mutation::ViewOption::unavailable(
                        "unsupported",
                    ),
                    security_invoker: lazydb::db::catalog_mutation::ViewOption::unavailable(
                        "unsupported",
                    ),
                    check_option: lazydb::db::catalog_mutation::ViewOption::unavailable(
                        "unsupported",
                    ),
                    baseline_fingerprint: String::new(),
                };
                lazydb::model::catalog_editor::ViewDraft::from_definition(&definition)
            }),
            None,
        )
        .unwrap();
    database.execute_catalog_mutation(&view_plan).await.unwrap();

    let capabilities = database.catalog_mutation_capabilities();
    assert!(
        capabilities
            .create
            .iter()
            .any(|item| item.object_type == CatalogObjectType::Catalog(CatalogKind::Schema))
    );
    assert_eq!(
        capabilities
            .edit
            .iter()
            .map(|option| option.object_type)
            .collect::<Vec<_>>(),
        vec![
            CatalogObjectType::Catalog(CatalogKind::Table),
            CatalogObjectType::Catalog(CatalogKind::View),
        ]
    );
    let schemas = database
        .execute("SELECT schema_name FROM duckdb_schemas() WHERE schema_name = 'sales space'")
        .await
        .unwrap();
    assert_eq!(schemas.result_sets[0].rows.len(), 1);
    let schema_entry = lazydb::db::catalog::CatalogEntry::schema(
        CatalogId::new(profile.id, CatalogKind::Schema, ["memory", "sales space"]),
        CatalogId::new(profile.id, CatalogKind::Database, ["memory"]),
        lazydb::db::catalog::QualifiedName {
            database: Some("memory".into()),
            schema: Some("sales space".into()),
            object: "sales space".into(),
        },
        "schema",
        lazydb::db::catalog::OptionalMetadata::Unsupported,
        true,
    )
    .unwrap();
    let drop_request =
        lazydb::db::catalog_drop::CatalogDropRequest::new(identity, schema_entry.id.clone(), 4);
    let schema_drop = database
        .plan_catalog_drop(drop_request, &schema_entry)
        .unwrap();
    assert!(schema_drop.sql().contains("DROP SCHEMA"));
    assert!(database.execute_catalog_drop(&schema_drop).await.is_err());
    assert_eq!(
        database
            .execute("SELECT count(*) FROM duckdb_schemas() WHERE schema_name = 'sales space'")
            .await
            .unwrap()
            .result_sets[0]
            .rows[0][0],
        lazydb::db::value::CellValue::Integer(1)
    );
    let table_id = CatalogId::new(
        profile.id,
        CatalogKind::Table,
        ["memory", "sales space", "order items"],
    );
    let table_entry = lazydb::db::catalog::CatalogEntry::relation(
        table_id.clone(),
        schema_entry.id.clone(),
        lazydb::db::catalog::QualifiedName {
            database: Some("memory".into()),
            schema: Some("sales space".into()),
            object: "order items".into(),
        },
        "BASE TABLE",
        lazydb::db::catalog::OptionalMetadata::Unsupported,
        true,
    )
    .unwrap();
    let table_drop = database
        .plan_catalog_drop(
            lazydb::db::catalog_drop::CatalogDropRequest::new(identity, table_id, 5),
            &table_entry,
        )
        .unwrap();
    assert!(table_drop.sql().contains("\"sales space\".\"order items\""));
    database.execute_catalog_drop(&table_drop).await.unwrap();

    let stale_id = CatalogId::new(
        profile.id,
        CatalogKind::Table,
        ["memory", "sales space", "order items"],
    );
    let stale_entry = lazydb::db::catalog::CatalogEntry::relation(
        stale_id.clone(),
        schema_entry.id.clone(),
        lazydb::db::catalog::QualifiedName {
            database: Some("memory".into()),
            schema: Some("sales space".into()),
            object: "order items".into(),
        },
        "BASE TABLE",
        lazydb::db::catalog::OptionalMetadata::Unsupported,
        true,
    )
    .unwrap();
    let stale_plan = database
        .plan_catalog_drop(
            lazydb::db::catalog_drop::CatalogDropRequest::new(identity, stale_id, 6),
            &stale_entry,
        )
        .unwrap();
    assert!(database.execute_catalog_drop(&stale_plan).await.is_err());
    database.close().await;
}

#[tokio::test]
async fn duckdb_sequence_and_index_structured_create_definition_and_drop_round_trip() {
    use lazydb::db::catalog_mutation::{
        CatalogMutationAvailability, CatalogObjectDefinitionRequest,
    };

    let profile = import_connection_url("duckdb::memory:", Some("sequence-index-gates"))
        .unwrap()
        .profile;
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    database
        .execute("CREATE SEQUENCE seq_one START 7; CREATE TABLE indexed_rows (id INTEGER, value VARCHAR); CREATE INDEX idx_rows_id ON indexed_rows(id)")
        .await
        .unwrap();

    let capabilities = database.catalog_mutation_capabilities();
    for kind in [CatalogKind::Sequence, CatalogKind::Index] {
        assert_eq!(
            database.catalog_drop_availability(kind),
            lazydb::db::catalog_mutation::CatalogMutationAvailability::Available
        );
        assert_eq!(
            capabilities.create_availability(CatalogObjectType::Catalog(kind)),
            Some(CatalogMutationAvailability::Available)
        );
        assert_eq!(
            capabilities.edit_availability(CatalogObjectType::Catalog(kind)),
            None
        );
    }

    let target = ExecutionTarget::from_profile(&profile);
    for (kind, name) in [
        (CatalogKind::Sequence, "seq_one"),
        (CatalogKind::Index, "idx_rows_id"),
    ] {
        let object = if kind == CatalogKind::Sequence {
            CatalogId::new(profile.id, kind, ["memory", "main", name])
        } else {
            CatalogId::new(profile.id, kind, ["memory", "main", "indexed_rows", name])
        };
        let definition_request = CatalogObjectDefinitionRequest {
            connection: ConnectionIdentity {
                profile_id: profile.id,
                generation: 1,
            },
            request_id: 10,
            catalog_epoch: 0,
            object: object.clone(),
            target: target.clone(),
            principal: None,
        };
        let definition_result = database
            .load_catalog_object_definition(&definition_request)
            .await;
        if kind == CatalogKind::Sequence {
            let definition = definition_result.unwrap();
            assert!(matches!(
                definition,
                lazydb::db::catalog_mutation::CatalogObjectDefinition::Sequence(_)
            ));
        } else {
            let definition = definition_result.unwrap();
            assert!(matches!(
                definition,
                lazydb::db::catalog_mutation::CatalogObjectDefinition::Index(_)
            ));
        }
        let qualified_name = lazydb::db::catalog::QualifiedName {
            database: Some("memory".into()),
            schema: Some("main".into()),
            object: name.into(),
        };
        let drop_entry = if kind == CatalogKind::Index {
            lazydb::db::catalog::CatalogEntry::relation_child(
                object.clone(),
                CatalogId::new(
                    profile.id,
                    CatalogKind::Table,
                    ["memory", "main", "indexed_rows"],
                ),
                qualified_name,
                "INDEX",
                lazydb::db::catalog::OptionalMetadata::Unsupported,
                lazydb::db::catalog::CatalogMetadata::Index(lazydb::db::catalog::IndexMetadata {
                    columns: Vec::new(),
                    unique: false,
                }),
            )
            .unwrap()
        } else {
            lazydb::db::catalog::CatalogEntry::object(
                object.clone(),
                CatalogId::new(profile.id, CatalogKind::Schema, ["memory", "main"]),
                qualified_name,
                "SEQUENCE",
                lazydb::db::catalog::OptionalMetadata::Unsupported,
                false,
            )
            .unwrap()
        };
        let drop_request = lazydb::db::catalog_drop::CatalogDropRequest::new(
            definition_request.connection,
            object,
            11,
        );
        let drop_plan = database
            .plan_catalog_drop(drop_request, &drop_entry)
            .unwrap();
        assert!(drop_plan.sql().contains(&format!("\"{name}\"")));
        database.execute_catalog_drop(&drop_plan).await.unwrap();
        let remaining = if kind == CatalogKind::Sequence {
            database
                .execute("SELECT count(*) FROM duckdb_sequences() WHERE sequence_name = 'seq_one'")
                .await
                .unwrap()
        } else {
            database
                .execute("SELECT count(*) FROM duckdb_indexes() WHERE index_name = 'idx_rows_id'")
                .await
                .unwrap()
        };
        assert_eq!(
            remaining.result_sets[0].rows[0][0],
            lazydb::db::value::CellValue::Integer(0)
        );
    }

    database
        .execute("CREATE SEQUENCE seq_definition START 11 INCREMENT 3 MINVALUE 1 MAXVALUE 50 CYCLE")
        .await
        .unwrap();
    let sequence_object = CatalogId::new(
        profile.id,
        CatalogKind::Sequence,
        ["memory", "main", "seq_definition"],
    );
    let sequence_definition = database
        .load_catalog_object_definition(&CatalogObjectDefinitionRequest {
            connection: ConnectionIdentity {
                profile_id: profile.id,
                generation: 1,
            },
            request_id: 20,
            catalog_epoch: 0,
            object: sequence_object.clone(),
            target: target.clone(),
            principal: None,
        })
        .await
        .unwrap();
    let lazydb::db::catalog_mutation::CatalogObjectDefinition::Sequence(sequence) =
        &sequence_definition
    else {
        unreachable!()
    };
    let mut sequence_draft =
        lazydb::model::catalog_editor::SequenceDraft::from_definition(sequence);
    sequence_draft.name = "seq_created".into();
    sequence_draft.start_value = "19".into();
    let sequence_plan = database
        .plan_catalog_mutation(
            CatalogMutationRequest {
                connection: ConnectionIdentity {
                    profile_id: profile.id,
                    generation: 1,
                },
                request_id: 21,
                catalog_epoch: 0,
                mode: CatalogMutationMode::Create,
                anchor: CatalogMutationAnchor::Group {
                    schema: CatalogId::new(profile.id, CatalogKind::Schema, ["memory", "main"]),
                    group: lazydb::db::catalog::ObjectGroup::Sequences,
                },
                object_type: CatalogObjectType::Catalog(CatalogKind::Sequence),
                current_database: Some("memory".into()),
            },
            CatalogDraft::Sequence(sequence_draft),
            None,
        )
        .unwrap();
    database
        .execute_catalog_mutation(&sequence_plan)
        .await
        .unwrap();
    let created_sequence = match &sequence_plan.selection {
        lazydb::db::catalog_mutation::CatalogSelectionHint::Object(id) => id.clone(),
        _ => unreachable!(),
    };
    let created_definition = database
        .load_catalog_object_definition(&CatalogObjectDefinitionRequest {
            connection: ConnectionIdentity {
                profile_id: profile.id,
                generation: 1,
            },
            request_id: 22,
            catalog_epoch: 0,
            object: created_sequence.clone(),
            target: target.clone(),
            principal: None,
        })
        .await
        .unwrap();
    let lazydb::db::catalog_mutation::CatalogObjectDefinition::Sequence(
        created_sequence_definition,
    ) = created_definition
    else {
        unreachable!()
    };
    assert_eq!(created_sequence_definition.start_value, "19");
    assert_eq!(created_sequence_definition.increment, "3");

    database
        .execute("CREATE INDEX idx_rows_id ON indexed_rows(id)")
        .await
        .unwrap();

    let index_definition = database
        .load_catalog_object_definition(&CatalogObjectDefinitionRequest {
            connection: ConnectionIdentity {
                profile_id: profile.id,
                generation: 1,
            },
            request_id: 30,
            catalog_epoch: 0,
            object: CatalogId::new(
                profile.id,
                CatalogKind::Index,
                ["memory", "main", "indexed_rows", "idx_rows_id"],
            ),
            target: target.clone(),
            principal: None,
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "index identity {:?} failed: {error:?}",
                ["memory", "main", "indexed_rows", "idx_rows_id"]
            )
        });
    let lazydb::db::catalog_mutation::CatalogObjectDefinition::Index(index) = index_definition
    else {
        unreachable!()
    };
    let mut index_draft = lazydb::model::catalog_editor::IndexDraft::from_definition(&index);
    index_draft.name = "idx_rows_value".into();
    index_draft.columns[0].expression = "value".into();
    let index_plan = database
        .plan_catalog_mutation(
            CatalogMutationRequest {
                connection: ConnectionIdentity {
                    profile_id: profile.id,
                    generation: 1,
                },
                request_id: 31,
                catalog_epoch: 0,
                mode: CatalogMutationMode::Create,
                anchor: CatalogMutationAnchor::Group {
                    schema: CatalogId::new(profile.id, CatalogKind::Schema, ["memory", "main"]),
                    group: lazydb::db::catalog::ObjectGroup::Tables,
                },
                object_type: CatalogObjectType::Catalog(CatalogKind::Index),
                current_database: Some("memory".into()),
            },
            CatalogDraft::Index(index_draft),
            None,
        )
        .unwrap();
    database
        .execute_catalog_mutation(&index_plan)
        .await
        .unwrap();
    let index_id = match &index_plan.selection {
        lazydb::db::catalog_mutation::CatalogSelectionHint::Object(id) => id.clone(),
        _ => unreachable!(),
    };
    let index_definition = database
        .load_catalog_object_definition(&CatalogObjectDefinitionRequest {
            connection: ConnectionIdentity {
                profile_id: profile.id,
                generation: 1,
            },
            request_id: 32,
            catalog_epoch: 0,
            object: index_id.clone(),
            target: target.clone(),
            principal: None,
        })
        .await
        .unwrap();
    let lazydb::db::catalog_mutation::CatalogObjectDefinition::Index(index) = index_definition
    else {
        unreachable!()
    };
    assert_eq!(index.name, "idx_rows_value");
    assert_eq!(index.columns[0].expression, "'\"value\"'");
    database.close().await;
}

#[tokio::test]
async fn duckdb_table_view_rename_edits_round_trip_and_reject_stale_baselines() {
    use lazydb::db::catalog_mutation::{CatalogObjectDefinition, CatalogObjectDefinitionRequest};
    let mut profile = import_connection_url("duckdb::memory:", Some("catalog-edit"))
        .unwrap()
        .profile;
    profile.catalog_scope = lazydb::profile::CatalogScope::for_profile(
        lazydb::profile::DatabaseKind::DuckDb,
        "memory",
        None,
    );
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    database.execute("CREATE SCHEMA \"sales space\"; CREATE TABLE \"sales space\".\"order items\" (id INTEGER NOT NULL); CREATE VIEW \"sales space\".\"order view\" AS SELECT id FROM \"sales space\".\"order items\"").await.unwrap();
    let identity = ConnectionIdentity {
        profile_id: profile.id,
        generation: 8,
    };
    let target = ExecutionTarget::from_profile(&profile);
    let capabilities = database.catalog_mutation_capabilities();
    assert!(
        capabilities
            .edit
            .iter()
            .any(|option| option.object_type == CatalogObjectType::Catalog(CatalogKind::Table))
    );
    assert!(
        capabilities
            .edit
            .iter()
            .any(|option| option.object_type == CatalogObjectType::Catalog(CatalogKind::View))
    );
    assert!(
        !capabilities
            .edit
            .iter()
            .any(|option| option.object_type == CatalogObjectType::Catalog(CatalogKind::Schema))
    );
    for (kind, old_name, new_name) in [
        (CatalogKind::Table, "order items", "renamed items"),
        (CatalogKind::View, "order view", "renamed view"),
    ] {
        let object = CatalogId::new(profile.id, kind, ["memory", "sales space", old_name]);
        let definition = database
            .load_catalog_object_definition(&CatalogObjectDefinitionRequest {
                connection: identity,
                request_id: 1,
                catalog_epoch: 0,
                object: object.clone(),
                target: target.clone(),
                principal: None,
            })
            .await
            .unwrap();
        let draft = match &definition {
            CatalogObjectDefinition::Table(table) => {
                let mut draft =
                    lazydb::model::catalog_editor::TableDraft::from_definition_for_database(
                        table,
                        lazydb::profile::DatabaseKind::DuckDb,
                    );
                draft.name = new_name.into();
                CatalogDraft::Table(draft)
            }
            CatalogObjectDefinition::View(view) => {
                let mut draft = lazydb::model::catalog_editor::ViewDraft::from_definition(view);
                draft.name = new_name.into();
                CatalogDraft::View(draft)
            }
            _ => unreachable!(),
        };
        let plan = database
            .plan_catalog_mutation(
                CatalogMutationRequest {
                    connection: identity,
                    request_id: 2,
                    catalog_epoch: 0,
                    mode: CatalogMutationMode::Edit,
                    anchor: CatalogMutationAnchor::Catalog(object.clone()),
                    object_type: CatalogObjectType::Catalog(kind),
                    current_database: Some("memory".into()),
                },
                draft,
                Some(definition.clone()),
            )
            .unwrap();
        assert!(plan.statements()[0].contains("\"sales space\""));
        assert!(plan.statements()[0].contains(&format!("\"{old_name}\"")));
        database.execute_catalog_mutation(&plan).await.unwrap();
        let exists = database.execute(&format!("SELECT count(*) FROM duckdb_tables() WHERE table_name = '{new_name}' UNION ALL SELECT count(*) FROM duckdb_views() WHERE view_name = '{new_name}'")).await.unwrap();
        assert!(
            exists.result_sets[0]
                .rows
                .iter()
                .flatten()
                .any(|value| *value == lazydb::db::value::CellValue::Integer(1))
        );

        let stale_definition = match definition {
            CatalogObjectDefinition::Table(mut table) => {
                table.baseline_fingerprint.push_str("-stale");
                CatalogObjectDefinition::Table(table)
            }
            CatalogObjectDefinition::View(mut view) => {
                view.baseline_fingerprint.push_str("-stale");
                CatalogObjectDefinition::View(view)
            }
            _ => unreachable!(),
        };
        let stale_draft = match &stale_definition {
            CatalogObjectDefinition::Table(table) => {
                let mut draft =
                    lazydb::model::catalog_editor::TableDraft::from_definition_for_database(
                        table,
                        lazydb::profile::DatabaseKind::DuckDb,
                    );
                draft.name = format!("{new_name} stale").into();
                CatalogDraft::Table(draft)
            }
            CatalogObjectDefinition::View(view) => {
                let mut draft = lazydb::model::catalog_editor::ViewDraft::from_definition(view);
                draft.name = format!("{new_name} stale").into();
                CatalogDraft::View(draft)
            }
            _ => unreachable!(),
        };
        let stale_plan = database
            .plan_catalog_mutation(
                CatalogMutationRequest {
                    connection: identity,
                    request_id: 3,
                    catalog_epoch: 0,
                    mode: CatalogMutationMode::Edit,
                    anchor: CatalogMutationAnchor::Catalog(CatalogId::new(
                        profile.id,
                        kind,
                        ["memory", "sales space", new_name],
                    )),
                    object_type: CatalogObjectType::Catalog(kind),
                    current_database: Some("memory".into()),
                },
                stale_draft,
                Some(stale_definition),
            )
            .unwrap();
        assert!(
            database
                .execute_catalog_mutation(&stale_plan)
                .await
                .is_err()
        );
    }

    database.close().await;
}

#[tokio::test]
async fn duckdb_view_query_edit_round_trips_and_rejects_stale_baseline() {
    use lazydb::db::catalog_mutation::{CatalogObjectDefinition, CatalogObjectDefinitionRequest};

    let mut profile = import_connection_url("duckdb::memory:", Some("view-query-edit"))
        .unwrap()
        .profile;
    profile.catalog_scope = lazydb::profile::CatalogScope::for_profile(
        lazydb::profile::DatabaseKind::DuckDb,
        "memory",
        None,
    );
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    database
        .execute("CREATE VIEW main.\"answer view\" AS SELECT 41 AS answer")
        .await
        .unwrap();
    let identity = ConnectionIdentity {
        profile_id: profile.id,
        generation: 12,
    };
    let object = CatalogId::new(
        profile.id,
        CatalogKind::View,
        ["memory", "main", "answer view"],
    );
    let target = ExecutionTarget::from_profile(&profile);
    let definition = database
        .load_catalog_object_definition(&CatalogObjectDefinitionRequest {
            connection: identity,
            request_id: 1,
            catalog_epoch: 0,
            object: object.clone(),
            target: target.clone(),
            principal: None,
        })
        .await
        .unwrap();
    let CatalogObjectDefinition::View(view) = &definition else {
        panic!("expected view definition");
    };
    let mut draft = lazydb::model::catalog_editor::ViewDraft::from_definition(view);
    draft.query = "SELECT 42 AS answer".into();
    let plan = database
        .plan_catalog_mutation(
            CatalogMutationRequest {
                connection: identity,
                request_id: 2,
                catalog_epoch: 0,
                mode: CatalogMutationMode::Edit,
                anchor: CatalogMutationAnchor::Catalog(object.clone()),
                object_type: CatalogObjectType::Catalog(CatalogKind::View),
                current_database: Some("memory".into()),
            },
            CatalogDraft::View(draft),
            Some(definition.clone()),
        )
        .unwrap();
    assert!(plan.statements()[0].contains("CREATE OR REPLACE VIEW \"main\".\"answer view\""));
    database.execute_catalog_mutation(&plan).await.unwrap();
    let current = database
        .execute("SELECT answer FROM main.\"answer view\"")
        .await
        .unwrap();
    assert_eq!(
        current.result_sets[0].rows[0][0],
        lazydb::db::value::CellValue::Integer(42)
    );

    let mut stale_definition = definition;
    let CatalogObjectDefinition::View(view) = &mut stale_definition else {
        unreachable!();
    };
    view.baseline_fingerprint.push_str("-stale");
    let mut stale_draft = lazydb::model::catalog_editor::ViewDraft::from_definition(view);
    stale_draft.query = "SELECT 43 AS answer".into();
    let stale_plan = database
        .plan_catalog_mutation(
            CatalogMutationRequest {
                connection: identity,
                request_id: 3,
                catalog_epoch: 0,
                mode: CatalogMutationMode::Edit,
                anchor: CatalogMutationAnchor::Catalog(object),
                object_type: CatalogObjectType::Catalog(CatalogKind::View),
                current_database: Some("memory".into()),
            },
            CatalogDraft::View(stale_draft),
            Some(stale_definition),
        )
        .unwrap();
    assert!(
        database
            .execute_catalog_mutation(&stale_plan)
            .await
            .is_err()
    );
    let unchanged = database
        .execute("SELECT answer FROM main.\"answer view\"")
        .await
        .unwrap();
    assert_eq!(
        unchanged.result_sets[0].rows[0][0],
        lazydb::db::value::CellValue::Integer(42)
    );
    database.close().await;
}

#[tokio::test]
async fn duckdb_view_query_edit_rejects_external_baseline_change_without_overwrite() {
    use lazydb::db::catalog_mutation::{CatalogObjectDefinition, CatalogObjectDefinitionRequest};

    let mut profile = import_connection_url("duckdb::memory:", Some("view-stale-query-edit"))
        .unwrap()
        .profile;
    profile.catalog_scope = lazydb::profile::CatalogScope::for_profile(
        lazydb::profile::DatabaseKind::DuckDb,
        "memory",
        None,
    );
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    database
        .execute("CREATE VIEW main.current_view AS SELECT 41 AS answer")
        .await
        .unwrap();
    let identity = ConnectionIdentity {
        profile_id: profile.id,
        generation: 13,
    };
    let object = CatalogId::new(
        profile.id,
        CatalogKind::View,
        ["memory", "main", "current_view"],
    );
    let target = ExecutionTarget::from_profile(&profile);
    let definition = database
        .load_catalog_object_definition(&CatalogObjectDefinitionRequest {
            connection: identity,
            request_id: 1,
            catalog_epoch: 0,
            object: object.clone(),
            target,
            principal: None,
        })
        .await
        .unwrap();
    let CatalogObjectDefinition::View(view) = &definition else {
        unreachable!();
    };
    let mut stale_draft = lazydb::model::catalog_editor::ViewDraft::from_definition(view);
    stale_draft.query = "SELECT 42 AS answer".into();
    let stale_plan = database
        .plan_catalog_mutation(
            CatalogMutationRequest {
                connection: identity,
                request_id: 2,
                catalog_epoch: 0,
                mode: CatalogMutationMode::Edit,
                anchor: CatalogMutationAnchor::Catalog(object),
                object_type: CatalogObjectType::Catalog(CatalogKind::View),
                current_database: Some("memory".into()),
            },
            CatalogDraft::View(stale_draft),
            Some(definition),
        )
        .unwrap();

    database
        .execute("CREATE OR REPLACE VIEW main.current_view AS SELECT 43 AS answer")
        .await
        .unwrap();
    assert!(
        database
            .execute_catalog_mutation(&stale_plan)
            .await
            .is_err()
    );
    let current = database
        .execute("SELECT answer FROM main.current_view")
        .await
        .unwrap();
    assert_eq!(
        current.result_sets[0].rows[0][0],
        lazydb::db::value::CellValue::Integer(43)
    );
    database.close().await;
}

#[tokio::test]
async fn duckdb_table_edit_adds_nullable_column_and_rejects_stale_baseline() {
    use lazydb::db::catalog_mutation::{CatalogObjectDefinition, CatalogObjectDefinitionRequest};

    let mut profile = import_connection_url("duckdb::memory:", Some("table-add-column"))
        .unwrap()
        .profile;
    profile.catalog_scope = lazydb::profile::CatalogScope::for_profile(
        lazydb::profile::DatabaseKind::DuckDb,
        "memory",
        None,
    );
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    database
        .execute("CREATE TABLE main.items (id INTEGER NOT NULL, label VARCHAR NOT NULL); INSERT INTO main.items VALUES (1, 'preserved')")
        .await
        .unwrap();
    let identity = ConnectionIdentity {
        profile_id: profile.id,
        generation: 14,
    };
    let object = CatalogId::new(profile.id, CatalogKind::Table, ["memory", "main", "items"]);
    let target = ExecutionTarget::from_profile(&profile);
    let definition = database
        .load_catalog_object_definition(&CatalogObjectDefinitionRequest {
            connection: identity,
            request_id: 1,
            catalog_epoch: 0,
            object: object.clone(),
            target: target.clone(),
            principal: None,
        })
        .await
        .unwrap();
    let CatalogObjectDefinition::Table(table) = &definition else {
        unreachable!()
    };
    let mut draft = lazydb::model::catalog_editor::TableDraft::from_definition_for_database(
        table,
        lazydb::profile::DatabaseKind::DuckDb,
    );
    let mut added = lazydb::model::catalog_editor::ColumnDraft::new_added_for_database(
        lazydb::profile::DatabaseKind::DuckDb,
    );
    added.name = "note".into();
    added.native_type = "VARCHAR".into();
    added.nullable = true;
    draft.columns.push(added);
    let plan = database
        .plan_catalog_mutation(
            CatalogMutationRequest {
                connection: identity,
                request_id: 2,
                catalog_epoch: 0,
                mode: CatalogMutationMode::Edit,
                anchor: CatalogMutationAnchor::Catalog(object.clone()),
                object_type: CatalogObjectType::Catalog(CatalogKind::Table),
                current_database: Some("memory".into()),
            },
            CatalogDraft::Table(draft),
            Some(definition.clone()),
        )
        .unwrap();
    assert!(plan.statements()[0].contains("ADD COLUMN \"note\" VARCHAR"));
    database.execute_catalog_mutation(&plan).await.unwrap();
    let result = database
        .execute("SELECT id, label, note FROM main.items")
        .await
        .unwrap();
    assert_eq!(
        result.result_sets[0].rows[0][0],
        lazydb::db::value::CellValue::Integer(1)
    );
    assert_eq!(
        result.result_sets[0].rows[0][1],
        lazydb::db::value::CellValue::Text("preserved".into())
    );
    assert_eq!(
        result.result_sets[0].rows[0][2],
        lazydb::db::value::CellValue::Null
    );

    let CatalogObjectDefinition::Table(mut stale_table) = definition else {
        unreachable!()
    };
    stale_table.baseline_fingerprint.push_str("-stale");
    let mut stale_draft = lazydb::model::catalog_editor::TableDraft::from_definition_for_database(
        &stale_table,
        lazydb::profile::DatabaseKind::DuckDb,
    );
    let mut stale_added = lazydb::model::catalog_editor::ColumnDraft::new_added_for_database(
        lazydb::profile::DatabaseKind::DuckDb,
    );
    stale_added.name = "other".into();
    stale_added.native_type = "INTEGER".into();
    stale_draft.columns.push(stale_added);
    let stale_plan = database
        .plan_catalog_mutation(
            CatalogMutationRequest {
                connection: identity,
                request_id: 3,
                catalog_epoch: 0,
                mode: CatalogMutationMode::Edit,
                anchor: CatalogMutationAnchor::Catalog(object),
                object_type: CatalogObjectType::Catalog(CatalogKind::Table),
                current_database: Some("memory".into()),
            },
            CatalogDraft::Table(stale_draft),
            Some(CatalogObjectDefinition::Table(stale_table)),
        )
        .unwrap();
    assert!(
        database
            .execute_catalog_mutation(&stale_plan)
            .await
            .is_err()
    );
    let unchanged = database
        .execute("SELECT count(*) FROM duckdb_columns() WHERE table_name = 'items' AND column_name = 'other'")
        .await
        .unwrap();
    assert_eq!(
        unchanged.result_sets[0].rows[0][0],
        lazydb::db::value::CellValue::Integer(0)
    );
    database.close().await;
}

#[tokio::test]
async fn duckdb_table_edit_renames_column_and_rejects_external_stale_baseline() {
    use lazydb::db::catalog_mutation::{CatalogObjectDefinition, CatalogObjectDefinitionRequest};

    let mut profile = import_connection_url("duckdb::memory:", Some("table-rename-column"))
        .unwrap()
        .profile;
    profile.catalog_scope = lazydb::profile::CatalogScope::for_profile(
        lazydb::profile::DatabaseKind::DuckDb,
        "memory",
        None,
    );
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    database
        .execute("CREATE TABLE main.items (id INTEGER PRIMARY KEY, label VARCHAR NOT NULL DEFAULT 'original'); INSERT INTO main.items VALUES (1, DEFAULT)")
        .await
        .unwrap();
    let identity = ConnectionIdentity {
        profile_id: profile.id,
        generation: 15,
    };
    let target = ExecutionTarget::from_profile(&profile);
    let object = CatalogId::new(profile.id, CatalogKind::Table, ["memory", "main", "items"]);
    let load = |request_id| CatalogObjectDefinitionRequest {
        connection: identity,
        request_id,
        catalog_epoch: 0,
        object: object.clone(),
        target: target.clone(),
        principal: None,
    };
    let definition = database
        .load_catalog_object_definition(&load(1))
        .await
        .unwrap();
    let CatalogObjectDefinition::Table(table) = &definition else {
        unreachable!()
    };
    let mut draft = lazydb::model::catalog_editor::TableDraft::from_definition_for_database(
        table,
        lazydb::profile::DatabaseKind::DuckDb,
    );
    draft.columns[1].name = "description".into();
    let plan = database
        .plan_catalog_mutation(
            CatalogMutationRequest {
                connection: identity,
                request_id: 2,
                catalog_epoch: 0,
                mode: CatalogMutationMode::Edit,
                anchor: CatalogMutationAnchor::Catalog(object.clone()),
                object_type: CatalogObjectType::Catalog(CatalogKind::Table),
                current_database: Some("memory".into()),
            },
            CatalogDraft::Table(draft),
            Some(definition),
        )
        .unwrap();
    assert!(plan.statements()[0].contains("RENAME COLUMN \"label\" TO \"description\""));
    database.execute_catalog_mutation(&plan).await.unwrap();

    let renamed_object =
        CatalogId::new(profile.id, CatalogKind::Table, ["memory", "main", "items"]);
    let reloaded = database
        .load_catalog_object_definition(&load(3))
        .await
        .unwrap();
    let CatalogObjectDefinition::Table(reloaded_table) = &reloaded else {
        unreachable!()
    };
    assert_eq!(reloaded_table.columns[1].name, "description");
    assert!(!reloaded_table.columns[1].nullable);
    assert_eq!(
        reloaded_table.columns[1].default_expression,
        lazydb::db::catalog::OptionalMetadata::Supported(Some("'original'".into()))
    );
    let preserved = database
        .execute("SELECT id, description FROM main.items")
        .await
        .unwrap();
    assert_eq!(
        preserved.result_sets[0].rows[0][0],
        lazydb::db::value::CellValue::Integer(1)
    );
    assert_eq!(
        preserved.result_sets[0].rows[0][1],
        lazydb::db::value::CellValue::Text("original".into())
    );

    let mut stale_draft = lazydb::model::catalog_editor::TableDraft::from_definition_for_database(
        reloaded_table,
        lazydb::profile::DatabaseKind::DuckDb,
    );
    stale_draft.columns[1].name = "stale_description".into();
    let stale_plan = database
        .plan_catalog_mutation(
            CatalogMutationRequest {
                connection: identity,
                request_id: 4,
                catalog_epoch: 0,
                mode: CatalogMutationMode::Edit,
                anchor: CatalogMutationAnchor::Catalog(renamed_object.clone()),
                object_type: CatalogObjectType::Catalog(CatalogKind::Table),
                current_database: Some("memory".into()),
            },
            CatalogDraft::Table(stale_draft),
            Some(reloaded.clone()),
        )
        .unwrap();
    database
        .execute("ALTER TABLE main.items ADD COLUMN externally_added INTEGER")
        .await
        .unwrap();
    assert!(
        database
            .execute_catalog_mutation(&stale_plan)
            .await
            .is_err()
    );
    let final_definition = database
        .load_catalog_object_definition(&load(5))
        .await
        .unwrap();
    let CatalogObjectDefinition::Table(final_table) = final_definition else {
        unreachable!()
    };
    assert_eq!(final_table.columns[1].name, "description");
    assert_eq!(final_table.columns[2].name, "externally_added");
    database.close().await;
}

#[tokio::test]
async fn duckdb_table_edit_changes_default_and_nullability_with_round_trip() {
    use lazydb::db::catalog_mutation::{CatalogObjectDefinition, CatalogObjectDefinitionRequest};

    let mut profile = import_connection_url("duckdb::memory:", Some("table-alter-column"))
        .unwrap()
        .profile;
    profile.catalog_scope = lazydb::profile::CatalogScope::for_profile(
        lazydb::profile::DatabaseKind::DuckDb,
        "memory",
        None,
    );
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    database.execute("CREATE TABLE main.flags (id INTEGER, label VARCHAR DEFAULT 'old'); INSERT INTO main.flags (id) VALUES (1)").await.unwrap();
    let identity = ConnectionIdentity {
        profile_id: profile.id,
        generation: 16,
    };
    let object = CatalogId::new(profile.id, CatalogKind::Table, ["memory", "main", "flags"]);
    let target = ExecutionTarget::from_profile(&profile);
    let load = |request_id| CatalogObjectDefinitionRequest {
        connection: identity,
        request_id,
        catalog_epoch: 0,
        object: object.clone(),
        target: target.clone(),
        principal: None,
    };
    let definition = database
        .load_catalog_object_definition(&load(1))
        .await
        .unwrap();
    let CatalogObjectDefinition::Table(table) = &definition else {
        unreachable!()
    };
    let mut draft = lazydb::model::catalog_editor::TableDraft::from_definition_for_database(
        table,
        lazydb::profile::DatabaseKind::DuckDb,
    );
    draft.columns[1].nullable = false;
    draft.columns[1].default_expression = "'new'".into();
    let plan = database
        .plan_catalog_mutation(
            CatalogMutationRequest {
                connection: identity,
                request_id: 2,
                catalog_epoch: 0,
                mode: CatalogMutationMode::Edit,
                anchor: CatalogMutationAnchor::Catalog(object.clone()),
                object_type: CatalogObjectType::Catalog(CatalogKind::Table),
                current_database: Some("memory".into()),
            },
            CatalogDraft::Table(draft),
            Some(definition),
        )
        .unwrap();
    database.execute_catalog_mutation(&plan).await.unwrap();
    let reloaded = database
        .load_catalog_object_definition(&load(3))
        .await
        .unwrap();
    let CatalogObjectDefinition::Table(table) = reloaded else {
        unreachable!()
    };
    assert!(!table.columns[1].nullable);
    assert_eq!(
        table.columns[1].default_expression,
        lazydb::db::catalog::OptionalMetadata::Supported(Some("'new'".into()))
    );
    let result = database
        .execute("SELECT id, label FROM main.flags")
        .await
        .unwrap();
    assert_eq!(
        result.result_sets[0].rows[0][1],
        lazydb::db::value::CellValue::Text("old".into())
    );

    let definition = database
        .load_catalog_object_definition(&load(4))
        .await
        .unwrap();
    let CatalogObjectDefinition::Table(table) = &definition else {
        unreachable!()
    };
    let mut draft = lazydb::model::catalog_editor::TableDraft::from_definition_for_database(
        table,
        lazydb::profile::DatabaseKind::DuckDb,
    );
    draft.columns[1].nullable = true;
    draft.columns[1].default_expression = String::new().into();
    let plan = database
        .plan_catalog_mutation(
            CatalogMutationRequest {
                connection: identity,
                request_id: 5,
                catalog_epoch: 0,
                mode: CatalogMutationMode::Edit,
                anchor: CatalogMutationAnchor::Catalog(object.clone()),
                object_type: CatalogObjectType::Catalog(CatalogKind::Table),
                current_database: Some("memory".into()),
            },
            CatalogDraft::Table(draft),
            Some(definition),
        )
        .unwrap();
    database.execute_catalog_mutation(&plan).await.unwrap();
    let reloaded = database
        .load_catalog_object_definition(&load(6))
        .await
        .unwrap();
    let CatalogObjectDefinition::Table(table) = reloaded else {
        unreachable!()
    };
    assert!(table.columns[1].nullable);
    assert_eq!(
        table.columns[1].default_expression,
        lazydb::db::catalog::OptionalMetadata::Supported(None)
    );
    let result = database
        .execute("INSERT INTO main.flags (id) VALUES (2) RETURNING id, label")
        .await
        .unwrap();
    assert_eq!(
        result.result_sets[0].rows[0][1],
        lazydb::db::value::CellValue::Null
    );
    database.close().await;
}

#[tokio::test]
async fn duckdb_table_edit_changes_column_type_and_drops_column_without_rebuild() {
    use lazydb::db::catalog_mutation::{CatalogObjectDefinition, CatalogObjectDefinitionRequest};

    let mut profile = import_connection_url("duckdb::memory:", Some("table-column-shape"))
        .unwrap()
        .profile;
    profile.catalog_scope = lazydb::profile::CatalogScope::for_profile(
        lazydb::profile::DatabaseKind::DuckDb,
        "memory",
        None,
    );
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    database.execute("CREATE TABLE main.metrics (id INTEGER PRIMARY KEY, reading INTEGER, obsolete VARCHAR); INSERT INTO main.metrics VALUES (1, 7, 'keep row')").await.unwrap();
    let identity = ConnectionIdentity {
        profile_id: profile.id,
        generation: 17,
    };
    let object = CatalogId::new(
        profile.id,
        CatalogKind::Table,
        ["memory", "main", "metrics"],
    );
    let target = ExecutionTarget::from_profile(&profile);
    let load = |request_id| CatalogObjectDefinitionRequest {
        connection: identity,
        request_id,
        catalog_epoch: 0,
        object: object.clone(),
        target: target.clone(),
        principal: None,
    };
    let definition = database
        .load_catalog_object_definition(&load(1))
        .await
        .unwrap();
    let CatalogObjectDefinition::Table(table) = &definition else {
        unreachable!()
    };
    let mut draft = lazydb::model::catalog_editor::TableDraft::from_definition_for_database(
        table,
        lazydb::profile::DatabaseKind::DuckDb,
    );
    draft.columns[1].native_type = "BIGINT".into();
    draft.remove_column_row(draft.columns[2].row_id);
    let plan = database
        .plan_catalog_mutation(
            CatalogMutationRequest {
                connection: identity,
                request_id: 2,
                catalog_epoch: 0,
                mode: CatalogMutationMode::Edit,
                anchor: CatalogMutationAnchor::Catalog(object.clone()),
                object_type: CatalogObjectType::Catalog(CatalogKind::Table),
                current_database: Some("memory".into()),
            },
            CatalogDraft::Table(draft),
            Some(definition),
        )
        .unwrap();
    database.execute_catalog_mutation(&plan).await.unwrap();
    let reloaded = database
        .load_catalog_object_definition(&load(3))
        .await
        .unwrap();
    let CatalogObjectDefinition::Table(table) = reloaded else {
        unreachable!()
    };
    assert_eq!(table.columns.len(), 2);
    assert_eq!(table.columns[1].name, "reading");
    assert_eq!(table.columns[1].native_type, "BIGINT");
    let rows = database
        .execute("SELECT id, reading FROM main.metrics")
        .await
        .unwrap();
    assert_eq!(
        rows.result_sets[0].rows[0][0],
        lazydb::db::value::CellValue::Integer(1)
    );
    assert_eq!(
        rows.result_sets[0].rows[0][1],
        lazydb::db::value::CellValue::Integer(7)
    );
    database.close().await;
}

#[tokio::test]
async fn duckdb_native_probe_reports_index_and_comment_ddl_support() {
    let profile = import_connection_url("duckdb::memory:", Some("native-mutation-probe"))
        .unwrap()
        .profile;
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    database.execute("CREATE TABLE main.probe (id INTEGER, note VARCHAR); CREATE VIEW main.probe_view AS SELECT * FROM main.probe; CREATE INDEX probe_idx ON main.probe(id); COMMENT ON TABLE main.probe IS 'table note'; COMMENT ON COLUMN main.probe.note IS 'column note'; COMMENT ON INDEX probe_idx IS 'index note'; COMMENT ON VIEW main.probe_view IS 'view note'").await.unwrap();
    let index_ddl = database
        .object_ddl(CatalogKind::Index, "main", "probe_idx")
        .await
        .unwrap();
    assert!(
        index_ddl.is_none(),
        "DuckDB does not expose replayable index SQL"
    );
    let table_comment = database
        .execute("SELECT comment FROM duckdb_tables() WHERE table_name = 'probe'")
        .await
        .unwrap();
    assert_eq!(
        table_comment.result_sets[0].rows[0][0],
        lazydb::db::value::CellValue::Text("table note".into())
    );
    let column_comment = database.execute("SELECT comment FROM duckdb_columns() WHERE table_name = 'probe' AND column_name = 'note'").await.unwrap();
    assert_eq!(
        column_comment.result_sets[0].rows[0][0],
        lazydb::db::value::CellValue::Text("column note".into())
    );
    let view_comment = database
        .execute("SELECT comment FROM duckdb_views() WHERE view_name = 'probe_view'")
        .await
        .unwrap();
    assert_eq!(
        view_comment.result_sets[0].rows[0][0],
        lazydb::db::value::CellValue::Text("view note".into())
    );
    database.close().await;
}

#[tokio::test]
async fn duckdb_schema_and_type_drop_use_native_drop_plans() {
    let profile = import_connection_url("duckdb::memory:", Some("schema-type-drop"))
        .unwrap()
        .profile;
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    database
        .execute("CREATE SCHEMA disposable; CREATE TYPE main.mood AS ENUM ('happy', 'sad')")
        .await
        .unwrap();
    for kind in [CatalogKind::Schema, CatalogKind::Type] {
        assert_eq!(
            database.catalog_drop_availability(kind),
            lazydb::db::catalog_mutation::CatalogMutationAvailability::Available
        );
    }
    let schema_id = CatalogId::new(profile.id, CatalogKind::Schema, ["memory", "disposable"]);
    let schema_entry = lazydb::db::catalog::CatalogEntry::schema(
        schema_id.clone(),
        CatalogId::new(profile.id, CatalogKind::Database, ["memory"]),
        lazydb::db::catalog::QualifiedName {
            database: Some("memory".into()),
            schema: Some("disposable".into()),
            object: "disposable".into(),
        },
        "schema",
        lazydb::db::catalog::OptionalMetadata::Unsupported,
        true,
    )
    .unwrap();
    let schema_plan = database
        .plan_catalog_drop(
            lazydb::db::catalog_drop::CatalogDropRequest::new(
                ConnectionIdentity {
                    profile_id: profile.id,
                    generation: 1,
                },
                schema_id,
                1,
            ),
            &schema_entry,
        )
        .unwrap();
    assert!(database.execute_catalog_drop(&schema_plan).await.is_ok());

    let type_id = CatalogId::new(profile.id, CatalogKind::Type, ["memory", "main", "mood"]);
    let type_entry = lazydb::db::catalog::CatalogEntry::object(
        type_id.clone(),
        CatalogId::new(profile.id, CatalogKind::Schema, ["memory", "main"]),
        lazydb::db::catalog::QualifiedName {
            database: Some("memory".into()),
            schema: Some("main".into()),
            object: "mood".into(),
        },
        "TYPE",
        lazydb::db::catalog::OptionalMetadata::Unsupported,
        false,
    )
    .unwrap();
    let type_plan = database
        .plan_catalog_drop(
            lazydb::db::catalog_drop::CatalogDropRequest::new(
                ConnectionIdentity {
                    profile_id: profile.id,
                    generation: 1,
                },
                type_id,
                2,
            ),
            &type_entry,
        )
        .unwrap();
    assert!(database.execute_catalog_drop(&type_plan).await.is_ok());
    database.close().await;
}

#[tokio::test]
async fn duckdb_table_definition_loads_authoritative_comments_indexes_and_constraints() {
    use lazydb::db::catalog_mutation::{CatalogObjectDefinition, CatalogObjectDefinitionRequest};
    let profile = import_connection_url("duckdb::memory:", Some("constraint-probe"))
        .unwrap()
        .profile;
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    database.execute("CREATE TABLE main.probe (id INTEGER PRIMARY KEY, code VARCHAR UNIQUE, amount INTEGER CHECK(amount > 0), parent INTEGER, FOREIGN KEY(parent) REFERENCES main.probe(id)); CREATE INDEX probe_code_idx ON main.probe(code); COMMENT ON TABLE main.probe IS 'table'; COMMENT ON COLUMN main.probe.code IS 'column'").await.unwrap();
    let constraints = database
        .execute("SELECT * FROM duckdb_constraints() WHERE table_name='probe'")
        .await
        .unwrap();
    assert_eq!(constraints.result_sets[0].rows.len(), 5);
    let indexes = database
        .execute("SELECT * FROM duckdb_indexes() WHERE table_name='probe'")
        .await
        .unwrap();
    assert_eq!(indexes.result_sets[0].rows.len(), 1);
    let mut profile = profile;
    profile.catalog_scope = lazydb::profile::CatalogScope::for_profile(
        lazydb::profile::DatabaseKind::DuckDb,
        "memory",
        None,
    );
    let identity = ConnectionIdentity {
        profile_id: profile.id,
        generation: 31,
    };
    let target = ExecutionTarget::from_profile(&profile);
    let definition = database
        .load_catalog_object_definition(&CatalogObjectDefinitionRequest {
            connection: identity,
            request_id: 1,
            catalog_epoch: 0,
            object: CatalogId::new(profile.id, CatalogKind::Table, ["memory", "main", "probe"]),
            target,
            principal: None,
        })
        .await
        .unwrap();
    let CatalogObjectDefinition::Table(table) = definition else {
        unreachable!()
    };
    assert_eq!(
        table.comment,
        lazydb::db::catalog::OptionalMetadata::Supported(Some("table".into()))
    );
    assert_eq!(
        table.columns[1].comment,
        lazydb::db::catalog::OptionalMetadata::Supported(Some("column".into()))
    );
    assert_eq!(table.indexes.len(), 1);
    assert_eq!(table.constraints.len(), 4);
    assert_eq!(table.structured_constraints.len(), 4);
    database.close().await;
}

#[tokio::test]
async fn duckdb_structured_table_constraints_create_load_enforce_and_reject_unsupported_options() {
    use lazydb::db::catalog_mutation::ConstraintDefinitionKind;
    use lazydb::db::catalog_mutation::{
        CatalogMutationAnchor, CatalogObjectDefinition, CatalogObjectDefinitionRequest,
    };
    use lazydb::model::catalog_editor::CatalogDraft;
    use lazydb::model::catalog_editor::{ConstraintDraft, TableDraft};

    let mut profile = import_connection_url("duckdb::memory:", Some("table-constraints"))
        .unwrap()
        .profile;
    profile.catalog_scope = lazydb::profile::CatalogScope::for_profile(
        lazydb::profile::DatabaseKind::DuckDb,
        "memory",
        None,
    );
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    database
        .execute("CREATE TABLE main.parent (id INTEGER PRIMARY KEY)")
        .await
        .unwrap();
    let identity = ConnectionIdentity {
        profile_id: profile.id,
        generation: 41,
    };
    let target = ExecutionTarget::from_profile(&profile);
    let object = CatalogId::new(profile.id, CatalogKind::Table, ["memory", "main", "child"]);
    let mut draft = TableDraft::new_for_database("main", lazydb::profile::DatabaseKind::DuckDb);
    draft.name = "child".into();
    draft.columns[0].name = "id".into();
    draft.columns[0].native_type = "INTEGER".into();
    draft.columns.push(
        lazydb::model::catalog_editor::ColumnDraft::new_added_for_database(
            lazydb::profile::DatabaseKind::DuckDb,
        ),
    );
    draft.columns[1].name = "code".into();
    draft.columns[1].native_type = "VARCHAR".into();
    draft.columns.push(
        lazydb::model::catalog_editor::ColumnDraft::new_added_for_database(
            lazydb::profile::DatabaseKind::DuckDb,
        ),
    );
    draft.columns[2].name = "parent_id".into();
    draft.columns[2].native_type = "INTEGER".into();
    for (name, kind, columns) in [
        (
            "child_pk",
            ConstraintDefinitionKind::PrimaryKey { columns: vec![] },
            "id",
        ),
        (
            "child_unique",
            ConstraintDefinitionKind::Unique { columns: vec![] },
            "code",
        ),
    ] {
        let mut constraint = ConstraintDraft::new(kind, "main", "child");
        constraint.name = name.into();
        constraint.columns = columns.into();
        draft.add_constraint(constraint);
    }
    let mut check = ConstraintDraft::new(
        ConstraintDefinitionKind::Check {
            expression: String::new(),
            no_inherit: false,
        },
        "main",
        "child",
    );
    check.name = "child_code_check".into();
    check.expression = "length(code) > 0".into();
    draft.add_constraint(check);
    let mut foreign = ConstraintDraft::new(
        ConstraintDefinitionKind::ForeignKey {
            columns: vec![],
            referenced_schema: String::new(),
            referenced_relation: String::new(),
            referenced_columns: vec![],
            match_type: "SIMPLE".into(),
            on_update: "NO ACTION".into(),
            on_delete: "NO ACTION".into(),
        },
        "main",
        "child",
    );
    foreign.name = "child_parent_fk".into();
    foreign.columns = "parent_id".into();
    foreign.referenced_schema = "main".into();
    foreign.referenced_relation = "parent".into();
    foreign.referenced_columns = "id".into();
    draft.add_constraint(foreign);
    let plan = database
        .plan_catalog_mutation(
            CatalogMutationRequest {
                connection: identity,
                request_id: 1,
                catalog_epoch: 0,
                mode: CatalogMutationMode::Create,
                anchor: CatalogMutationAnchor::Catalog(CatalogId::new(
                    profile.id,
                    CatalogKind::Schema,
                    ["memory", "main"],
                )),
                object_type: CatalogObjectType::Catalog(CatalogKind::Table),
                current_database: Some("memory".into()),
            },
            CatalogDraft::Table(draft.clone()),
            None,
        )
        .unwrap();
    assert!(plan.statements()[0].contains("PRIMARY KEY"));
    assert!(plan.statements()[0].contains("UNIQUE"));
    assert!(plan.statements()[0].contains("CHECK"));
    assert!(plan.statements()[0].contains("FOREIGN KEY"));
    database.execute_catalog_mutation(&plan).await.unwrap();
    let definition = database
        .load_catalog_object_definition(&CatalogObjectDefinitionRequest {
            connection: identity,
            request_id: 2,
            catalog_epoch: 0,
            object: object.clone(),
            target,
            principal: None,
        })
        .await
        .unwrap();
    let CatalogObjectDefinition::Table(table) = definition else {
        unreachable!()
    };
    assert_eq!(table.structured_constraints.len(), 4);
    assert!(
        table
            .structured_constraints
            .iter()
            .any(|constraint| matches!(
                constraint.kind,
                ConstraintDefinitionKind::PrimaryKey { .. }
            ))
    );
    assert!(
        table
            .structured_constraints
            .iter()
            .any(|constraint| matches!(constraint.kind, ConstraintDefinitionKind::Unique { .. }))
    );
    assert!(
        table
            .structured_constraints
            .iter()
            .any(|constraint| matches!(constraint.kind, ConstraintDefinitionKind::Check { .. }))
    );
    assert!(
        table
            .structured_constraints
            .iter()
            .any(|constraint| matches!(
                constraint.kind,
                ConstraintDefinitionKind::ForeignKey { .. }
            ))
    );
    database
        .execute("INSERT INTO main.parent VALUES (1); INSERT INTO main.child VALUES (1, 'ok', 1)")
        .await
        .unwrap();
    assert!(
        database
            .execute("INSERT INTO main.child VALUES (2, '', 1)")
            .await
            .is_err()
    );
    assert!(
        database
            .execute("INSERT INTO main.child VALUES (3, 'ok', 99)")
            .await
            .is_err()
    );
    assert!(
        database
            .execute("INSERT INTO main.child VALUES (1, 'duplicate', 1)")
            .await
            .is_err()
    );

    let mut unsupported = draft.clone();
    unsupported.table_constraints[0].deferrable = true;
    let rejected = database.plan_catalog_mutation(
        CatalogMutationRequest {
            connection: identity,
            request_id: 3,
            catalog_epoch: 0,
            mode: CatalogMutationMode::Create,
            anchor: CatalogMutationAnchor::Catalog(CatalogId::new(
                profile.id,
                CatalogKind::Schema,
                ["memory", "main"],
            )),
            object_type: CatalogObjectType::Catalog(CatalogKind::Table),
            current_database: Some("memory".into()),
        },
        CatalogDraft::Table(unsupported),
        None,
    );
    assert!(rejected.is_err());
    let mut duplicate = draft.clone();
    duplicate.table_constraints[1].name = duplicate.table_constraints[0].name.clone();
    assert!(
        database
            .plan_catalog_mutation(
                CatalogMutationRequest {
                    connection: identity,
                    request_id: 4,
                    catalog_epoch: 0,
                    mode: CatalogMutationMode::Create,
                    anchor: CatalogMutationAnchor::Catalog(CatalogId::new(
                        profile.id,
                        CatalogKind::Schema,
                        ["memory", "main"]
                    )),
                    object_type: CatalogObjectType::Catalog(CatalogKind::Table),
                    current_database: Some("memory".into())
                },
                CatalogDraft::Table(duplicate),
                None,
            )
            .is_err()
    );
    database.close().await;
}

#[tokio::test]
async fn duckdb_table_and_view_comment_edits_round_trip_and_stale_changes_are_rejected() {
    use lazydb::db::catalog_mutation::{
        CatalogMutationAnchor, CatalogObjectDefinition, CatalogObjectDefinitionRequest,
    };
    let mut profile = import_connection_url("duckdb::memory:", Some("comment-edits"))
        .unwrap()
        .profile;
    profile.catalog_scope = lazydb::profile::CatalogScope::for_profile(
        lazydb::profile::DatabaseKind::DuckDb,
        "memory",
        None,
    );
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    database.execute("CREATE TABLE main.items (id INTEGER, note VARCHAR); CREATE VIEW main.item_view AS SELECT * FROM main.items; COMMENT ON TABLE main.items IS 'old table'; COMMENT ON COLUMN main.items.note IS 'old column'; COMMENT ON VIEW main.item_view IS 'old view'").await.unwrap();
    let identity = ConnectionIdentity {
        profile_id: profile.id,
        generation: 18,
    };
    let target = ExecutionTarget::from_profile(&profile);
    for (kind, name) in [
        (CatalogKind::Table, "items"),
        (CatalogKind::View, "item_view"),
    ] {
        let object = CatalogId::new(profile.id, kind, ["memory", "main", name]);
        let load = |request_id| CatalogObjectDefinitionRequest {
            connection: identity,
            request_id,
            catalog_epoch: 0,
            object: object.clone(),
            target: target.clone(),
            principal: None,
        };
        let definition = database
            .load_catalog_object_definition(&load(1))
            .await
            .unwrap();
        match &definition {
            CatalogObjectDefinition::Table(table) => {
                assert_eq!(
                    table.comment,
                    lazydb::db::catalog::OptionalMetadata::Supported(Some("old table".into()))
                );
                assert_eq!(
                    table.columns[1].comment,
                    lazydb::db::catalog::OptionalMetadata::Supported(Some("old column".into()))
                );
            }
            CatalogObjectDefinition::View(view) => {
                assert_eq!(
                    view.comment,
                    lazydb::db::catalog::OptionalMetadata::Supported(Some("old view".into()))
                );
            }
            _ => unreachable!(),
        }
        let draft = match &definition {
            CatalogObjectDefinition::Table(table) => {
                let mut draft =
                    lazydb::model::catalog_editor::TableDraft::from_definition_for_database(
                        table,
                        lazydb::profile::DatabaseKind::DuckDb,
                    );
                draft.comment = "new table".into();
                draft.columns[1].comment = "new column".into();
                lazydb::model::catalog_editor::CatalogDraft::Table(draft)
            }
            CatalogObjectDefinition::View(view) => {
                let mut draft = lazydb::model::catalog_editor::ViewDraft::from_definition(view);
                draft.comment = "new view".into();
                lazydb::model::catalog_editor::CatalogDraft::View(draft)
            }
            _ => unreachable!(),
        };
        let plan = database
            .plan_catalog_mutation(
                CatalogMutationRequest {
                    connection: identity,
                    request_id: 2,
                    catalog_epoch: 0,
                    mode: CatalogMutationMode::Edit,
                    anchor: CatalogMutationAnchor::Catalog(object.clone()),
                    object_type: CatalogObjectType::Catalog(kind),
                    current_database: Some("memory".into()),
                },
                draft,
                Some(definition.clone()),
            )
            .unwrap();
        database.execute_catalog_mutation(&plan).await.unwrap();
        let reloaded = database
            .load_catalog_object_definition(&load(3))
            .await
            .unwrap();
        match &reloaded {
            CatalogObjectDefinition::Table(table) => {
                assert_eq!(
                    table.comment,
                    lazydb::db::catalog::OptionalMetadata::Supported(Some("new table".into()))
                );
                assert_eq!(
                    table.columns[1].comment,
                    lazydb::db::catalog::OptionalMetadata::Supported(Some("new column".into()))
                );
            }
            CatalogObjectDefinition::View(view) => assert_eq!(
                view.comment,
                lazydb::db::catalog::OptionalMetadata::Supported(Some("new view".into()))
            ),
            _ => unreachable!(),
        }
        let stale_draft = match &reloaded {
            CatalogObjectDefinition::Table(table) => {
                let mut draft =
                    lazydb::model::catalog_editor::TableDraft::from_definition_for_database(
                        table,
                        lazydb::profile::DatabaseKind::DuckDb,
                    );
                draft.comment = "stale".into();
                lazydb::model::catalog_editor::CatalogDraft::Table(draft)
            }
            CatalogObjectDefinition::View(view) => {
                let mut draft = lazydb::model::catalog_editor::ViewDraft::from_definition(view);
                draft.comment = "stale".into();
                lazydb::model::catalog_editor::CatalogDraft::View(draft)
            }
            _ => unreachable!(),
        };
        let stale_plan = database
            .plan_catalog_mutation(
                CatalogMutationRequest {
                    connection: identity,
                    request_id: 4,
                    catalog_epoch: 0,
                    mode: CatalogMutationMode::Edit,
                    anchor: CatalogMutationAnchor::Catalog(object.clone()),
                    object_type: CatalogObjectType::Catalog(kind),
                    current_database: Some("memory".into()),
                },
                stale_draft,
                Some(reloaded),
            )
            .unwrap();
        database
            .execute(&format!(
                "COMMENT ON {} main.{name} IS 'external'",
                if kind == CatalogKind::View {
                    "VIEW"
                } else {
                    "TABLE"
                }
            ))
            .await
            .unwrap();
        assert!(
            database
                .execute_catalog_mutation(&stale_plan)
                .await
                .is_err()
        );
    }
    let comments = database.execute("SELECT comment FROM duckdb_tables() WHERE table_name='items' UNION ALL SELECT comment FROM duckdb_views() WHERE view_name='item_view'").await.unwrap();
    assert!(
        comments.result_sets[0]
            .rows
            .iter()
            .flatten()
            .any(|value| *value == lazydb::db::value::CellValue::Text("external".into()))
    );
    database.close().await;
}

#[tokio::test]
async fn duckdb_decodes_nested_values_with_stable_text() {
    use lazydb::db::value::CellValue;
    let profile = import_connection_url("duckdb::memory:", Some("nested"))
        .unwrap()
        .profile;
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    let result = database
        .execute("SELECT [1, 2, NULL] AS list_value, {'x': 1, 'y': 'two'} AS struct_value, MAP {'a': 1, 'b': 2} AS map_value, [1, 2]::INTEGER[2] AS array_value")
        .await
        .unwrap();
    let row = &result.result_sets[0].rows[0];
    assert_eq!(row[0], CellValue::Text("[1, 2, NULL]".into()));
    assert!(
        matches!(&row[1], CellValue::Text(value) if value.contains("x: 1") && value.contains("y: two"))
    );
    assert!(
        matches!(&row[2], CellValue::Text(value) if value.contains("a: 1") && value.contains("b: 2"))
    );
    assert_eq!(row[3], CellValue::Text("[1, 2]".into()));
    database.close().await;
}

#[tokio::test]
async fn read_only_duckdb_reopens_existing_file_and_rejects_writes() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("readonly.duckdb");
    let profile = import_connection_url(&format!("duckdb://{}", path.display()), None)
        .unwrap()
        .profile;
    let writer = DatabaseConnection::connect(&profile, None).await.unwrap();
    writer.execute("CREATE TABLE t (id INTEGER)").await.unwrap();
    writer.close().await;
    let mut read_only_profile = profile.clone();
    read_only_profile.read_only = true;
    let reader = DatabaseConnection::connect(&read_only_profile, None)
        .await
        .unwrap();
    assert_eq!(
        reader
            .execute("SELECT count(*) FROM t")
            .await
            .unwrap()
            .result_sets[0]
            .rows[0][0],
        lazydb::db::value::CellValue::Integer(0)
    );
    assert_eq!(
        reader
            .execute("INSERT INTO t VALUES (1)")
            .await
            .unwrap_err()
            .category,
        ErrorCategory::Sql
    );
    reader.close().await;
}

#[tokio::test]
async fn read_only_duckdb_does_not_create_a_missing_file() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("missing.duckdb");
    let mut profile = import_connection_url(&format!("duckdb://{}", path.display()), None)
        .unwrap()
        .profile;
    profile.read_only = true;
    assert!(DatabaseConnection::connect(&profile, None).await.is_err());
    assert!(!path.exists());
}

#[tokio::test]
async fn duckdb_profiles_with_same_basename_remain_isolated() {
    let first_dir = TempDir::new().unwrap();
    let second_dir = TempDir::new().unwrap();
    let first_path = first_dir.path().join("same.duckdb");
    let second_path = second_dir.path().join("same.duckdb");
    let first_profile = import_connection_url(&format!("duckdb://{}", first_path.display()), None)
        .unwrap()
        .profile;
    let second_profile =
        import_connection_url(&format!("duckdb://{}", second_path.display()), None)
            .unwrap()
            .profile;
    let first = DatabaseConnection::connect(&first_profile, None)
        .await
        .unwrap();
    first
        .execute("CREATE TABLE only_first (value INTEGER)")
        .await
        .unwrap();
    first.close().await;
    let second = DatabaseConnection::connect(&second_profile, None)
        .await
        .unwrap();
    assert!(second.execute("SELECT * FROM only_first").await.is_err());
    second.close().await;
}

#[tokio::test]
async fn duckdb_catalog_lists_schema_relations_and_columns() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("catalog.duckdb");
    let profile = import_connection_url(&format!("duckdb://{}", path.display()), None)
        .unwrap()
        .profile;
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    database
        .execute("CREATE TABLE events (id INTEGER PRIMARY KEY, payload VARCHAR); INSERT INTO events VALUES (1, 'one'), (2, 'two'), (3, 'three')")
        .await
        .unwrap();
    database
        .execute("CREATE TABLE events_2 (id INTEGER); CREATE TABLE events_3 (id INTEGER)")
        .await
        .unwrap();
    database
        .execute("CREATE VIEW events_view AS SELECT id, payload FROM events; CREATE SEQUENCE event_sequence; CREATE MACRO event_macro(x) AS x + 1")
        .await
        .unwrap();
    let generation = 1;
    let identity = ConnectionIdentity {
        profile_id: profile.id,
        generation,
    };
    let target = ExecutionTarget::from_profile(&profile);
    assert_eq!(target.database, "catalog");
    let request = |request_id, target| CatalogRequest {
        key: CatalogRequestKey {
            connection: identity,
            catalog_epoch: 0,
            request_id,
            target,
            cursor: None,
        },
        scope: profile.catalog_scope.clone(),
        page_size: 100,
    };
    let roots = database
        .load_catalog_page(&request(1, CatalogTarget::Databases))
        .await
        .unwrap();
    let database_id = roots.entries[0].id.clone();
    let schemas = database
        .load_catalog_page(&request(
            2,
            CatalogTarget::Schemas {
                database: database_id,
            },
        ))
        .await
        .unwrap();
    let schema = schemas
        .entries
        .iter()
        .find(|entry| entry.qualified_name.object == "main")
        .unwrap()
        .id
        .clone();
    let groups = database
        .load_catalog_page(&request(
            20,
            CatalogTarget::Groups {
                schema: schema.clone(),
            },
        ))
        .await
        .unwrap();
    assert!(
        groups
            .group_summaries
            .iter()
            .any(|summary| summary.group == lazydb::db::catalog::ObjectGroup::Tables)
    );
    database.execute("ATTACH ':memory:' AS attached_db; CREATE TABLE attached_db.main.attached_events (id INTEGER)").await.unwrap();
    let attached_roots = database
        .load_catalog_page(&request(21, CatalogTarget::Databases))
        .await
        .unwrap();
    assert!(
        attached_roots
            .entries
            .iter()
            .any(|entry| entry.qualified_name.object == "attached_db")
    );
    let attached_id = attached_roots
        .entries
        .iter()
        .find(|entry| entry.qualified_name.object == "attached_db")
        .unwrap()
        .id
        .clone();
    let attached_schemas = database
        .load_catalog_page(&request(
            22,
            CatalogTarget::Schemas {
                database: attached_id,
            },
        ))
        .await
        .unwrap();
    let attached_main_schema = attached_schemas
        .entries
        .iter()
        .find(|entry| entry.qualified_name.object == "main")
        .unwrap()
        .id
        .clone();
    let attached_tables = database
        .load_catalog_page(&request(
            23,
            CatalogTarget::Objects {
                schema: attached_main_schema,
                group: lazydb::db::catalog::ObjectGroup::Tables,
            },
        ))
        .await
        .unwrap();
    assert_eq!(
        attached_tables.entries[0]
            .qualified_name
            .database
            .as_deref(),
        Some("attached_db")
    );
    assert_eq!(
        attached_tables.entries[0].qualified_name.object,
        "attached_events"
    );
    assert!(
        attached_tables.entries[0]
            .id
            .native_path
            .starts_with(&["attached_db".into(), "main".into()])
    );
    let tables = database
        .load_catalog_page(&request(
            3,
            CatalogTarget::Objects {
                schema: schema.clone(),
                group: lazydb::db::catalog::ObjectGroup::Tables,
            },
        ))
        .await
        .unwrap();
    let table = tables
        .entries
        .iter()
        .find(|entry| entry.qualified_name.object == "events")
        .unwrap()
        .id
        .clone();
    let target = ExecutionTarget::from_profile(&profile);
    let definition = database
        .load_catalog_object_definition(&CatalogObjectDefinitionRequest {
            connection: identity,
            request_id: 5,
            catalog_epoch: 0,
            object: table.clone(),
            target: target.clone(),
            principal: None,
        })
        .await
        .unwrap();
    assert!(
        matches!(definition, lazydb::db::catalog_mutation::CatalogObjectDefinition::Table(ref table) if table.columns.len() == 2 && table.baseline_fingerprint.starts_with("duckdb:table:"))
    );
    let outside_scope = database
        .load_catalog_object_definition(&CatalogObjectDefinitionRequest {
            connection: identity,
            request_id: 6,
            catalog_epoch: 0,
            object: CatalogId::new(
                profile.id,
                lazydb::db::catalog::CatalogKind::Table,
                ["attached_db", "main", "foreign_table"],
            ),
            target: target.clone(),
            principal: None,
        })
        .await;
    assert!(outside_scope.is_err());
    let ddl = database
        .relation_ddl_with_scope(&table, &profile.catalog_scope)
        .await
        .unwrap();
    assert!(ddl.sql.contains("CREATE TABLE"));
    assert_eq!(
        ddl.provenance,
        lazydb::db::catalog::DdlProvenance::NativeCatalog
    );
    assert_eq!(
        tables.total_count,
        lazydb::db::catalog::CatalogCount::Exact(3)
    );
    let columns = database
        .load_catalog_page(&request(
            4,
            CatalogTarget::RelationChildren {
                relation: table.clone(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(columns.entries.len(), 2);
    assert_eq!(columns.entries[0].qualified_name.object, "id");
    assert_eq!(
        columns.entries[0].metadata,
        lazydb::db::catalog::CatalogMetadata::Column({
            let mut column = lazydb::db::catalog::ColumnMetadata::new(1, "INTEGER", false);
            column.type_family =
                lazydb::db::catalog::OptionalMetadata::Supported(Some("INTEGER".into()));
            column
                .constraint_memberships
                .push(lazydb::db::catalog::ConstraintMembership {
                    constraint_id: lazydb::db::catalog::CatalogId::new(
                        profile.id,
                        lazydb::db::catalog::CatalogKind::PrimaryKey,
                        ["catalog", "main", "events", "PRIMARY KEY"],
                    ),
                    ordinal_position: 1,
                });
            column
        })
    );
    assert_eq!(columns.entries[1].qualified_name.object, "payload");
    let preview = database
        .preview_relation_with_scope(
            &table,
            &profile.catalog_scope,
            &Default::default(),
            lazydb::model::pagination::PageRequest::first(lazydb::model::pagination::PageSize::Ten),
        )
        .await
        .unwrap();
    assert_eq!(preview.result.result_sets[0].columns.len(), 2);
    assert_eq!(preview.result.result_sets[0].rows.len(), 3);
    assert!(preview.sql.contains("\"catalog\".\"main\".\"events\""));
    let paged_preview = database
        .preview_relation_with_scope(
            &table,
            &profile.catalog_scope,
            &Default::default(),
            lazydb::model::pagination::PageRequest::first(lazydb::model::pagination::PageSize::Ten),
        )
        .await
        .unwrap();
    assert_eq!(paged_preview.result.result_sets[0].rows.len(), 3);
    let filtered_preview = database
        .preview_relation_with_scope(
            &table,
            &profile.catalog_scope,
            &lazydb::model::relation::RelationPreviewOptions {
                where_clause: Some("id >= 0".into()),
                order_by_clause: Some("id DESC".into()),
            },
            lazydb::model::pagination::PageRequest::first(lazydb::model::pagination::PageSize::Ten),
        )
        .await
        .unwrap();
    assert!(
        filtered_preview
            .sql
            .contains("WHERE id >= 0 ORDER BY id DESC")
    );
    assert_eq!(
        filtered_preview.result.result_sets[0].rows[0][0],
        lazydb::db::value::CellValue::Integer(3)
    );
    let counted = database
        .preview_relation_with_scope(
            &table,
            &profile.catalog_scope,
            &Default::default(),
            lazydb::model::pagination::PageRequest::last(
                lazydb::model::pagination::PageSize::Ten,
                20,
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        counted.pagination.total,
        lazydb::model::pagination::TotalRows::Exact(3)
    );
    let denied = database
        .preview_relation_with_scope(
            &table,
            &lazydb::profile::CatalogScope::for_profile(
                lazydb::profile::DatabaseKind::DuckDb,
                "another_catalog",
                Some("main"),
            ),
            &Default::default(),
            lazydb::model::pagination::PageRequest::first(lazydb::model::pagination::PageSize::Ten),
        )
        .await
        .unwrap_err();
    assert_eq!(denied.category, ErrorCategory::Unsupported);
    let search = database
        .search_catalog(&CatalogSearchRequest {
            connection: identity,
            session_id: 0,
            generation: 1,
            query: "event".into(),
            scope: profile.catalog_scope.clone(),
            object_scope: CatalogSearchObjectScope::RelationsOnly,
            limit: 10,
        })
        .await
        .unwrap();
    assert_eq!(search.hits.len(), 5);
    assert_eq!(
        search.hits[0].entry.qualified_name.object,
        "attached_events"
    );
    assert_eq!(search.hits[0].ancestors.len(), 2);
    let all_object_search = database
        .search_catalog(&CatalogSearchRequest {
            connection: identity,
            session_id: 0,
            generation: 1,
            query: "event".into(),
            scope: profile.catalog_scope.clone(),
            object_scope: CatalogSearchObjectScope::AllObjects,
            limit: 20,
        })
        .await
        .unwrap();
    assert!(
        all_object_search
            .hits
            .iter()
            .any(|hit| hit.entry.kind == lazydb::db::catalog::CatalogKind::Sequence)
    );
    assert!(
        all_object_search
            .hits
            .iter()
            .any(|hit| hit.entry.qualified_name.object == "attached_events"
                && hit.entry.qualified_name.database.as_deref() == Some("attached_db"))
    );
    assert!(
        all_object_search
            .hits
            .iter()
            .any(|hit| hit.entry.kind == lazydb::db::catalog::CatalogKind::Function)
    );
    assert!(
        all_object_search
            .hits
            .iter()
            .any(|hit| hit.entry.kind == lazydb::db::catalog::CatalogKind::View)
    );
    assert!(
        all_object_search
            .hits
            .iter()
            .all(|hit| hit.ancestors.len() == 2)
    );
    let restricted_scope = lazydb::profile::CatalogScope::for_profile(
        lazydb::profile::DatabaseKind::DuckDb,
        "catalog",
        Some("unselected_schema"),
    );
    let excluded_search = database
        .search_catalog(&CatalogSearchRequest {
            connection: identity,
            session_id: 0,
            generation: 1,
            query: "event".into(),
            scope: restricted_scope,
            object_scope: CatalogSearchObjectScope::AllObjects,
            limit: 20,
        })
        .await
        .unwrap();
    assert!(excluded_search.hits.is_empty());
    let mut paged_tables = Vec::new();
    let mut cursor = None;
    for request_id in 30..32 {
        let mut page_request = request(
            request_id,
            CatalogTarget::Objects {
                schema: schema.clone(),
                group: lazydb::db::catalog::ObjectGroup::Tables,
            },
        );
        page_request.page_size = 2;
        page_request.key.cursor = cursor;
        let page = database.load_catalog_page(&page_request).await.unwrap();
        paged_tables.extend(
            page.entries
                .iter()
                .map(|entry| entry.qualified_name.object.clone()),
        );
        cursor = page.next_cursor;
    }
    assert_eq!(paged_tables, vec!["events", "events_2", "events_3"]);
    assert_eq!(
        database
            .discover_catalog_scope()
            .await
            .unwrap()
            .databases
            .len(),
        4
    );
    database.close().await;
}

#[tokio::test]
async fn duckdb_temp_catalog_search_is_session_local() {
    let profile = import_connection_url("duckdb::memory:", Some("temp-catalog"))
        .unwrap()
        .profile;
    let first = DatabaseConnection::connect(&profile, None).await.unwrap();
    let second = first.duckdb_session_clone().unwrap().unwrap();
    let empty_discovery = first.discover_catalog_scope().await.unwrap();
    assert!(
        empty_discovery
            .databases
            .iter()
            .any(|database| database.name == "temp")
    );
    first
        .execute("CREATE TEMP TABLE session_only_marker (id INTEGER)")
        .await
        .unwrap();

    let first_discovery = first.discover_catalog_scope().await.unwrap();
    let second_discovery = second.discover_catalog_scope().await.unwrap();
    assert!(
        first_discovery
            .databases
            .iter()
            .any(|database| database.name == "temp")
    );
    assert!(
        second_discovery
            .databases
            .iter()
            .any(|database| database.name == "temp")
    );

    let connection = ConnectionIdentity {
        profile_id: profile.id,
        generation: 9,
    };
    let request = |scope| CatalogSearchRequest {
        connection,
        session_id: 0,
        generation: 9,
        query: "session_only_marker".into(),
        scope,
        object_scope: CatalogSearchObjectScope::AllObjects,
        limit: 10,
    };
    let first_hits = first
        .search_catalog(&request(profile.catalog_scope.clone()))
        .await
        .unwrap();
    let second_hits = second
        .search_catalog(&request(profile.catalog_scope.clone()))
        .await
        .unwrap();
    assert_eq!(first_hits.hits.len(), 1);
    assert_eq!(
        first_hits.hits[0].entry.qualified_name.database.as_deref(),
        Some("temp")
    );
    assert!(second_hits.hits.is_empty());
    first.close().await;
    second.close().await;
}
