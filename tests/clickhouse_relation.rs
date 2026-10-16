use lazydb::{
    db::{
        DatabaseConnection,
        catalog::{CatalogId, CatalogKind},
        query::RELATION_PREVIEW_LIMIT,
    },
    model::{
        data_query::DataQueryOptions,
        pagination::{PageRequest, PageSize},
    },
    profile::{CatalogScope, DatabaseKind, import_connection_url},
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};
use uuid::Uuid;

#[tokio::test]
async fn clickhouse_relation_preview_verifies_scope_quotes_names_and_uses_lookahead() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        for request_number in 0..7 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let count = stream.read(&mut chunk).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..count]);
                if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let end = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap();
            let body = String::from_utf8_lossy(&request[end + 4..]);
            let response = if request_number == 0 {
                assert!(body.contains("SELECT version()"));
                "25.8.1\tanalytics\tdefault\n".to_owned()
            } else if request_number == 1 {
                assert!(body.contains("SELECT engine FROM system.tables"));
                "engine\nString\nMergeTree\n".to_owned()
            } else if request_number == 2 {
                assert!(body.contains("SELECT * FROM `analytics`.`events`"));
                let mut rows = String::from("id\tvalue\nUInt64\tString\n");
                for row in 1..=11 {
                    rows.push_str(&format!("{row}\tv{row}\n"));
                }
                rows
            } else if request_number == 3 {
                assert!(body.contains("SELECT engine, comment FROM system.tables WHERE"));
                "engine\tcomment\nString\tString\nMergeTree\tfixture\n".to_owned()
            } else if request_number == 4 {
                assert!(body.contains("SHOW CREATE TABLE"));
                "statement\nString\nCREATE TABLE analytics.events (`id` UInt64) ENGINE = MergeTree ORDER BY id\n".to_owned()
            } else if request_number == 5 {
                assert!(body.contains("SELECT engine, comment FROM system.tables WHERE"));
                "engine\tcomment\nString\tString\nMergeTree\tfixture\n".to_owned()
            } else if request_number == 6 {
                assert!(body.contains("SELECT engine FROM system.tables"));
                "engine\nString\nMergeTree\n".to_owned()
            } else {
                panic!("unexpected request {}: {}", request_number, body);
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            )
            .unwrap();
        }
    });
    let profile_id = Uuid::new_v4();
    let imported = import_connection_url(
        &format!("clickhouse://127.0.0.1:{}/analytics", address.port()),
        Some("relation-test"),
    )
    .unwrap();
    let mut profile = imported.profile;
    profile.id = profile_id;
    let connection = DatabaseConnection::connect(&profile, None).await.unwrap();
    let relation = CatalogId::new(
        profile_id,
        CatalogKind::Table,
        ["analytics", "analytics", "events"],
    );
    let scope = CatalogScope::for_profile(DatabaseKind::ClickHouse, "analytics", None);
    let preview = connection
        .preview_relation_with_scope(
            &relation,
            &scope,
            &DataQueryOptions {
                where_clause: Some("id > 0".into()),
                order_by_clause: Some("id".into()),
            },
            PageRequest::first(PageSize::Ten),
        )
        .await
        .unwrap();
    assert_eq!(preview.result.result_sets[0].rows.len(), 10);
    assert_eq!(preview.pagination.visible_rows, 10);
    assert!(preview.pagination.has_next);
    assert_eq!(preview.result.stats.fetched_row_count, 11);
    assert_eq!(preview.row_versions, None);
    let ddl = connection
        .relation_ddl_with_scope(&relation, &scope)
        .await
        .unwrap();
    assert!(ddl.sql.contains("ENGINE = MergeTree"));
    assert_eq!(
        ddl.provenance,
        lazydb::db::catalog::DdlProvenance::NativeCatalog
    );
    let rebound = connection
        .resolve_relation_identity_with_scope(&relation, &scope)
        .await
        .unwrap();
    assert_eq!(rebound.unwrap().id, relation);
    let injection = connection
        .preview_relation_with_scope(
            &relation,
            &scope,
            &DataQueryOptions {
                where_clause: Some("1 = 1) UNION SELECT 1 --".into()),
                order_by_clause: None,
            },
            PageRequest::first(PageSize::Ten),
        )
        .await
        .unwrap_err();
    assert!(
        injection
            .message
            .contains("comments and multiple statements")
            || injection.message.contains("invalid preview clause")
            || injection.message.contains("invalid preview query shape"),
        "{}",
        injection.message
    );
    connection.close().await;
    server.join().unwrap();
    assert_eq!(RELATION_PREVIEW_LIMIT, 500);
}

#[tokio::test]
async fn clickhouse_table_definition_loads_native_metadata_and_fingerprints_canonical_ddl() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        for (expected, response) in [
            (
                "SELECT version()",
                "25.8.1\tanalytics\tdefault\n".to_owned(),
            ),
            (
                "SELECT name, type, position, default_kind",
                "name\ttype\tposition\tdefault_kind\tdefault_expression\tcomment\nString\tString\tUInt64\tString\tString\tString\nid\tUInt64\t1\tDEFAULT\t7\tidentifier\n".to_owned(),
            ),
            (
                "SELECT engine, comment FROM system.tables",
                "engine\tcomment\nString\tString\nMergeTree\tfixture\n".to_owned(),
            ),
            (
                "SHOW CREATE TABLE",
                "statement\nString\nCREATE TABLE analytics.events (`id` UInt64 DEFAULT 7 COMMENT 'identifier') ENGINE = MergeTree ORDER BY id\n".to_owned(),
            ),
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 2048];
            loop {
                let count = stream.read(&mut chunk).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..count]);
                if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let length = headers.lines().find_map(|line| line.strip_prefix("content-length: ")).and_then(|value| value.parse::<usize>().ok()).unwrap_or(0);
                    if request.len() >= end + 4 + length { break; }
                }
            }
            let end = request.windows(4).position(|window| window == b"\r\n\r\n").unwrap();
            let body = String::from_utf8_lossy(&request[end + 4..]);
            assert!(body.contains(expected), "expected {expected:?}, got {body:?}");
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
        }
    });
    let profile_id = Uuid::new_v4();
    let imported = import_connection_url(
        &format!("clickhouse://127.0.0.1:{}/analytics", address.port()),
        Some("definition-test"),
    )
    .unwrap();
    let mut profile = imported.profile;
    profile.id = profile_id;
    let connection = DatabaseConnection::connect(&profile, None).await.unwrap();
    let relation = CatalogId::new(
        profile_id,
        CatalogKind::Table,
        ["analytics", "analytics", "events"],
    );
    let definition = connection
        .load_catalog_object_definition(
            &lazydb::db::catalog_mutation::CatalogObjectDefinitionRequest {
                connection: lazydb::identity::ConnectionIdentity {
                    profile_id,
                    generation: 1,
                },
                request_id: 7,
                catalog_epoch: 1,
                object: relation,
                target: lazydb::model::execution_target::ExecutionTarget {
                    profile_id,
                    database: "analytics".into(),
                    schema: Some("analytics".into()),
                },
                principal: None,
            },
        )
        .await
        .unwrap();
    let lazydb::db::catalog_mutation::CatalogObjectDefinition::ClickHouseTable(definition) =
        definition
    else {
        panic!("native definition expected");
    };
    assert_eq!(definition.engine, "MergeTree");
    assert_eq!(definition.columns[0].default_kind, "DEFAULT");
    assert_eq!(definition.columns[0].default_expression, "7");
    assert_eq!(
        definition.columns[0].comment,
        lazydb::db::catalog::OptionalMetadata::Supported(Some("identifier".into()))
    );
    assert!(definition.canonical_ddl.contains("ORDER BY id"));
    assert!(definition.baseline_fingerprint.starts_with("sha256:"));
    connection.close().await;
    server.join().unwrap();
}

#[tokio::test]
async fn clickhouse_relation_definition_and_preview_allow_scoped_non_default_database() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        for expected in [
            "SELECT version()",
            "SELECT name, type, position, default_kind",
            "SELECT engine, comment FROM system.tables",
            "SHOW CREATE TABLE",
            "SELECT engine FROM system.tables",
            "SELECT * FROM `warehouse`.`events`",
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 2048];
            loop {
                let count = stream.read(&mut chunk).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..count]);
                if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let end = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap();
            let body = String::from_utf8_lossy(&request[end + 4..]);
            assert!(
                body.contains(expected),
                "expected {expected:?}, got {body:?}"
            );
            let response = if expected == "SELECT version()" {
                "25.8.1\tdefault\tdefault\n"
            } else if expected == "SELECT name, type, position, default_kind" {
                "name\ttype\tposition\tdefault_kind\tdefault_expression\tcomment\nString\tString\tUInt64\tString\tString\tString\nid\tUInt64\t1\t\t\t\n"
            } else if expected == "SELECT engine, comment FROM system.tables" {
                "engine\tcomment\nString\tString\nMergeTree\t\n"
            } else if expected == "SHOW CREATE TABLE" {
                "statement\nString\nCREATE TABLE warehouse.events (`id` UInt64) ENGINE = MergeTree ORDER BY id\n"
            } else if expected == "SELECT engine FROM system.tables" {
                "engine\nString\nMergeTree\n"
            } else {
                "id\nUInt64\n1\n"
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            )
            .unwrap();
        }
    });
    let profile_id = Uuid::new_v4();
    let imported = import_connection_url(
        &format!("clickhouse://127.0.0.1:{}/default", address.port()),
        Some("cross-db-test"),
    )
    .unwrap();
    let mut profile = imported.profile;
    profile.id = profile_id;
    profile.catalog_scope = lazydb::profile::CatalogScope {
        databases: lazydb::profile::CatalogSelection::All,
    };
    let connection = DatabaseConnection::connect(&profile, None).await.unwrap();
    let relation = CatalogId::new(
        profile_id,
        CatalogKind::Table,
        ["warehouse", "warehouse", "events"],
    );
    let scope = profile.catalog_scope.clone();
    let definition = connection
        .load_catalog_object_definition(
            &lazydb::db::catalog_mutation::CatalogObjectDefinitionRequest {
                connection: lazydb::identity::ConnectionIdentity {
                    profile_id,
                    generation: 1,
                },
                request_id: 8,
                catalog_epoch: 1,
                object: relation.clone(),
                target: lazydb::model::execution_target::ExecutionTarget {
                    profile_id,
                    database: "warehouse".into(),
                    schema: Some("warehouse".into()),
                },
                principal: None,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        definition,
        lazydb::db::catalog_mutation::CatalogObjectDefinition::ClickHouseTable(_)
    ));
    let preview = connection
        .preview_relation_with_scope(
            &relation,
            &scope,
            &DataQueryOptions::default(),
            PageRequest::first(PageSize::Ten),
        )
        .await
        .unwrap();
    assert_eq!(preview.result.result_sets[0].rows.len(), 1);
    connection.close().await;
    server.join().unwrap();
}

#[tokio::test]
async fn clickhouse_relation_resolve_total_counts_only_when_requested() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let count = stream.read(&mut chunk).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..count]);
                if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let end = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap();
            let body = String::from_utf8_lossy(&request[end + 4..]);
            let response = if body.contains("SELECT version()") {
                "25.8.1\tanalytics\tdefault\n".to_owned()
            } else if body.contains("SELECT engine FROM system.tables") {
                "engine\nString\nMergeTree\n".to_owned()
            } else if body.contains("SELECT count() FROM (") {
                assert!(body.contains("SELECT count() FROM (SELECT * FROM `analytics`.`events` WHERE id > 0) AS lazydb_count"), "{body}");
                "count()\nUInt64\n25\n".to_owned()
            } else {
                assert!(
                    body.contains(
                        "SELECT * FROM `analytics`.`events` WHERE id > 0 LIMIT 11 OFFSET 20"
                    ),
                    "{body}"
                );
                "id\nUInt64\n21\n22\n23\n24\n25\n".to_owned()
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            )
            .unwrap();
        }
    });
    let profile_id = Uuid::new_v4();
    let imported = import_connection_url(
        &format!("clickhouse://127.0.0.1:{}/analytics", address.port()),
        Some("relation-total-test"),
    )
    .unwrap();
    let mut profile = imported.profile;
    profile.id = profile_id;
    let connection = DatabaseConnection::connect(&profile, None).await.unwrap();
    let relation = CatalogId::new(
        profile_id,
        CatalogKind::Table,
        ["analytics", "analytics", "events"],
    );
    let scope = CatalogScope::for_profile(DatabaseKind::ClickHouse, "analytics", None);
    let preview = connection
        .preview_relation_with_scope(
            &relation,
            &scope,
            &DataQueryOptions {
                where_clause: Some("id > 0".into()),
                order_by_clause: None,
            },
            PageRequest {
                size: PageSize::Ten,
                offset: 0,
                resolve_total: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        preview.pagination.total,
        lazydb::model::pagination::TotalRows::Exact(25)
    );
    assert_eq!(preview.pagination.offset, 20);
    assert_eq!(preview.pagination.visible_rows, 5);
    assert!(!preview.pagination.has_next);
    assert_eq!(preview.result.result_sets[0].rows.len(), 5);
    connection.close().await;
    server.join().unwrap();
}
