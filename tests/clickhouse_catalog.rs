use lazydb::{
    db::{
        DatabaseConnection,
        catalog::{
            CatalogCursor, CatalogId, CatalogKind, CatalogRequest, CatalogRequestKey,
            CatalogSearchObjectScope, CatalogSearchRequest, CatalogTarget,
        },
    },
    identity::ConnectionIdentity,
    profile::{CatalogScope, CatalogSelection, DatabaseKind, DatabaseScope, import_connection_url},
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};
use uuid::Uuid;

fn request(
    profile_id: Uuid,
    target: CatalogTarget,
    scope: CatalogScope,
    page_size: usize,
    cursor: Option<CatalogCursor>,
) -> CatalogRequest {
    CatalogRequest {
        key: CatalogRequestKey {
            connection: ConnectionIdentity {
                profile_id,
                generation: 1,
            },
            catalog_epoch: 2,
            request_id: 3,
            target,
            cursor,
        },
        scope,
        page_size,
    }
}

#[tokio::test]
async fn mock_catalog_pages_bind_database_scope_and_advance_keyset() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        for _ in 0..18 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let count = stream.read(&mut chunk).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..count]);
                let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                let length = headers
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length: "))
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(0);
                if length > 0 && request.len() >= end + 4 + length {
                    break;
                }
            }
            let end = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
            let body = String::from_utf8_lossy(&request[end + 4..]);
            let response_body = if body.contains("SELECT version()") {
                "25.8.1\tanalytics\tdefault\n"
            } else if body.contains("system.databases") {
                assert!(
                    body.contains("ORDER BY lower(name), name LIMIT 2"),
                    "missing bounded native paging: {body}"
                );
                if body.contains("lower(name) >") {
                    assert!(body.contains("lower(name) = 'analytics' AND name > 'analytics'"));
                    "name\nString\nwarehouse\n"
                } else {
                    "name\nString\nanalytics\nString\nwarehouse\n"
                }
            } else if body.contains("FROM system.columns") {
                assert!(
                    body.contains("ORDER BY position, name LIMIT 2"),
                    "missing bounded column paging: {body}"
                );
                if body.contains("position > toUInt64('1')") {
                    "name\ttype\tposition\tdefault_kind\tdefault_expression\tcomment\tdata_compressed_bytes\tdata_uncompressed_bytes\tmarks_bytes\nString\tString\tUInt64\tString\tString\tString\tUInt64\tUInt64\tUInt64\nname\tString\t2\t\t\t\t11\t20\t4\n"
                } else {
                    "name\ttype\tposition\tdefault_kind\tdefault_expression\tcomment\tdata_compressed_bytes\tdata_uncompressed_bytes\tmarks_bytes\nString\tString\tUInt64\tString\tString\tString\tUInt64\tUInt64\tUInt64\nid\tUInt64\t1\t\t\tfixture comment\t10\t18\t3\nname\tString\t2\t\t\t\t11\t20\t4\n"
                }
            } else if body.contains("FROM system.projections") {
                assert!(
                    body.contains("LIMIT 2"),
                    "projection page must be bounded: {body}"
                );
                if body.contains("lower(name) >") {
                    "name\tsorting_key\nString\tString\nprojection two\tvalue\n"
                } else {
                    "name\tsorting_key\nString\tString\nby_id\tid\nprojection two\tvalue\n"
                }
            } else if body.contains("SELECT count() FROM system.dictionaries") {
                "count()\nUInt64\n1\n"
            } else if body.contains("SELECT count() FROM system.functions") {
                "count()\nUInt64\n3\n"
            } else if body.contains("countIf(engine") {
                "tables\tviews\tmaterialized_views\nUInt64\tUInt64\tUInt64\n2\t0\t0\n"
            } else if body.contains("FROM system.dictionaries")
                && body.contains("ORDER BY lower(name)")
            {
                "name\tnative_kind\tcomment\nString\tString\tString\nlookup\tDICTIONARY\t\n"
            } else if body.contains("name ILIKE")
                && body.contains("lookup")
                && !body.contains("UNION ALL")
            {
                "database\tname\tengine\tcomment\nString\tString\tString\tString\nanalytics\tlookup\tDictionary\t\n"
            } else if body.contains("UNION ALL SELECT currentDatabase()")
                && body.contains("name ILIKE")
                && body.contains("LIMIT 11")
                && body.contains("my\\\\_func")
            {
                assert!(body.contains("currentDatabase()"));
                assert!(body.contains("LIMIT 11"));
                "database\tname\tnative_kind\tcomment\nString\tString\tString\tString\nanalytics\tmy_func\tSQLUserDefined\t\n"
            } else if body.contains("origin = 'SQLUserDefined'") && body.contains("count()") {
                "count()\nUInt64\n2\n"
            } else if body.contains("origin = 'SQLUserDefined'") && !body.contains("UNION ALL") {
                "name\tnative_kind\tcomment\nString\tString\tString\nmy_func\tSQLUserDefined\t\n"
            } else if body.contains("FROM system.functions")
                && body.contains("ORDER BY lower(name)")
            {
                "name\tnative_kind\tcomment\nString\tString\tString\nabs\tFUNCTION\t\n"
            } else if body.contains("UNION ALL SELECT database, name, 'DICTIONARY'")
                && body.contains("name ILIKE")
                && body.contains("LIMIT 2")
            {
                assert!(
                    body.contains("database = 'analytics'"),
                    "search scope missing from SQL: {body}"
                );
                assert!(body.contains("LIMIT 2"), "search lookahead missing: {body}");
                if body.contains("lookup") {
                    "database\tname\tnative_kind\tcomment\nString\tString\tString\tString\nanalytics\tlookup\tDICTIONARY\t\n"
                } else {
                    "database\tname\tnative_kind\tcomment\nString\tString\tString\tString\nanalytics\tFoo\tMergeTree\t\nanalytics\tfoo\tMergeTree\t\n"
                }
            } else if body.contains("UNION ALL SELECT database, name, 'DICTIONARY'")
                && body.contains("name ILIKE")
                && body.contains("LIMIT 11")
            {
                if body.contains("lookup") {
                    "database\tname\tnative_kind\tcomment\nString\tString\tString\tString\nanalytics\tlookup\tDICTIONARY\t\n"
                } else if body.contains("my\\\\_func") {
                    "database\tname\tnative_kind\tcomment\nString\tString\tString\tString\nanalytics\tmy_func\tSQLUserDefined\t\n"
                } else {
                    "database\tname\tnative_kind\tcomment\nString\tString\tString\tString\nanalytics\tFoo\tMergeTree\t\nanalytics\tfoo\tMergeTree\t\n"
                }
            } else if body.contains("lower(name) >") {
                "name\tengine\tcomment\nString\tString\tString\nfoo\tMergeTree\t\n"
            } else if body.contains("FROM system.tables") && !body.contains("UNION ALL") {
                assert!(
                    body.contains("LIMIT 2"),
                    "relation fetch must use server page_size+1: {body}"
                );
                "name\tengine\tcomment\nString\tString\tString\nFoo\tMergeTree\t\nfoo\tMergeTree\t\n"
            } else {
                assert!(
                    body.contains("database = 'analytics'"),
                    "unexpected catalog scope SQL: {body}"
                );
                panic!("unexpected catalog request: {body}");
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            )
            .unwrap();
        }
    });
    let profile_id = Uuid::new_v4();
    let url = format!("clickhouse://127.0.0.1:{}/analytics", address.port());
    let imported = import_connection_url(&url, Some("catalog-test")).unwrap();
    let mut profile = imported.profile;
    profile.id = profile_id;
    let connection = DatabaseConnection::connect(&profile, None).await.unwrap();
    let scope = CatalogScope {
        databases: CatalogSelection::Selected(vec![
            DatabaseScope {
                name: "analytics".into(),
                schemas: CatalogSelection::All,
            },
            DatabaseScope {
                name: "warehouse".into(),
                schemas: CatalogSelection::All,
            },
        ]),
    };
    let databases = connection
        .load_catalog_page(&request(
            profile_id,
            CatalogTarget::Databases,
            scope.clone(),
            1,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(databases.entries.len(), 1);
    assert_eq!(databases.entries[0].qualified_name.object, "analytics");
    let second_database_page = connection
        .load_catalog_page(&request(
            profile_id,
            CatalogTarget::Databases,
            scope.clone(),
            1,
            databases.next_cursor.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(
        second_database_page.entries[0].qualified_name.object,
        "warehouse"
    );
    let schema_id = CatalogId::new(profile_id, CatalogKind::Schema, ["analytics", "analytics"]);
    let groups = connection
        .load_catalog_page(&request(
            profile_id,
            CatalogTarget::groups(schema_id.clone()).unwrap(),
            scope.clone(),
            100,
            None,
        ))
        .await
        .unwrap();
    assert!(groups.group_summaries.iter().any(|summary| summary.group
        == lazydb::db::catalog::ObjectGroup::Dictionaries
        && summary.object_count == lazydb::db::catalog::CatalogCount::Exact(1)));
    let objects = connection
        .load_catalog_page(&request(
            profile_id,
            CatalogTarget::objects(schema_id.clone(), lazydb::db::catalog::ObjectGroup::Tables)
                .unwrap(),
            scope.clone(),
            1,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(objects.entries.len(), 1);
    assert!(matches!(
        objects.entries[0].qualified_name.object.as_str(),
        "Foo" | "foo"
    ));
    assert_eq!(
        objects.entries[0].id.native_path,
        [
            "analytics",
            "analytics",
            objects.entries[0].qualified_name.object.as_str()
        ]
    );
    let next_cursor = objects.next_cursor.clone().expect("next keyset cursor");
    let second_page = connection
        .load_catalog_page(&request(
            profile_id,
            CatalogTarget::objects(schema_id.clone(), lazydb::db::catalog::ObjectGroup::Tables)
                .unwrap(),
            CatalogScope::for_profile(DatabaseKind::ClickHouse, "analytics", None),
            1,
            Some(next_cursor),
        ))
        .await
        .unwrap();
    assert_eq!(second_page.entries[0].qualified_name.object, "foo");
    let relation_id = objects.entries[0].id.clone();
    let columns = connection
        .load_catalog_page(&request(
            profile_id,
            CatalogTarget::RelationChildren {
                relation: relation_id.clone(),
            },
            CatalogScope::for_profile(DatabaseKind::ClickHouse, "analytics", None),
            1,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(columns.entries[0].qualified_name.object, "id");
    let lazydb::db::catalog::CatalogMetadata::Column(column_metadata) =
        &columns.entries[0].metadata
    else {
        panic!("column page should retain typed metadata");
    };
    assert_eq!(
        column_metadata.comment,
        lazydb::db::catalog::OptionalMetadata::Supported(Some("fixture comment".into()))
    );
    assert_eq!(
        column_metadata.storage_statistics,
        lazydb::db::catalog::OptionalMetadata::Supported(Some(
            "compressed_bytes=10; uncompressed_bytes=18; marks_bytes=3".into()
        ))
    );
    let column_page = connection
        .load_catalog_page(&request(
            profile_id,
            CatalogTarget::RelationChildren {
                relation: relation_id.clone(),
            },
            CatalogScope::for_profile(DatabaseKind::ClickHouse, "analytics", None),
            1,
            columns.next_cursor,
        ))
        .await
        .unwrap();
    assert_eq!(column_page.entries[0].qualified_name.object, "name");
    let table_id = relation_id.clone();
    let projection_target = CatalogTarget::projection_children(table_id.clone()).unwrap();
    let projections = connection
        .load_catalog_page(&request(
            profile_id,
            projection_target.clone(),
            scope.clone(),
            1,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(projections.entries[0].kind, CatalogKind::Projection);
    assert_eq!(projections.entries[0].qualified_name.object, "by_id");
    assert!(!matches!(
        projections.entries[0].metadata,
        lazydb::db::catalog::CatalogMetadata::Index(lazydb::db::catalog::IndexMetadata {
            unique: true,
            ..
        })
    ));
    let projection_second = connection
        .load_catalog_page(&request(
            profile_id,
            projection_target,
            scope.clone(),
            1,
            projections.next_cursor.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(
        projection_second.entries[0].qualified_name.object,
        "projection two"
    );
    let mut mixed_request = request(
        profile_id,
        CatalogTarget::RelationChildren {
            relation: relation_id.clone(),
        },
        scope.clone(),
        10,
        None,
    );
    mixed_request.key.target = CatalogTarget::RelationChildren {
        relation: CatalogId::new(
            profile_id,
            CatalogKind::Projection,
            ["analytics", "analytics", "events", "by_id"],
        ),
    };
    assert!(mixed_request.validate().is_err());
    let dictionaries = connection
        .load_catalog_page(&request(
            profile_id,
            CatalogTarget::objects(
                schema_id.clone(),
                lazydb::db::catalog::ObjectGroup::Dictionaries,
            )
            .unwrap(),
            scope.clone(),
            10,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(dictionaries.entries[0].kind, CatalogKind::Dictionary);
    assert_eq!(dictionaries.entries[0].qualified_name.object, "lookup");
    let functions = connection
        .load_catalog_page(&request(
            profile_id,
            CatalogTarget::objects(
                schema_id.clone(),
                lazydb::db::catalog::ObjectGroup::UserDefinedFunctions,
            )
            .unwrap(),
            scope.clone(),
            10,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(functions.entries[0].kind, CatalogKind::UserDefinedFunction);
    assert_eq!(functions.entries[0].qualified_name.object, "my_func");
    let dictionary_search = connection
        .search_catalog(&CatalogSearchRequest {
            connection: ConnectionIdentity {
                profile_id,
                generation: 1,
            },
            session_id: 8,
            generation: 1,
            query: "lookup".into(),
            scope: scope.clone(),
            object_scope: CatalogSearchObjectScope::AllObjects,
            limit: 10,
        })
        .await
        .unwrap();
    assert_eq!(
        dictionary_search.hits[0].entry.kind,
        CatalogKind::Dictionary
    );
    let udf_search = connection
        .search_catalog(&CatalogSearchRequest {
            connection: ConnectionIdentity {
                profile_id,
                generation: 1,
            },
            session_id: 9,
            generation: 1,
            query: "my_func".into(),
            scope: scope.clone(),
            object_scope: CatalogSearchObjectScope::AllObjects,
            limit: 10,
        })
        .await
        .unwrap();
    assert_eq!(
        udf_search.hits[0].entry.kind,
        CatalogKind::UserDefinedFunction
    );
    let search = connection
        .search_catalog(&CatalogSearchRequest {
            connection: ConnectionIdentity {
                profile_id,
                generation: 1,
            },
            session_id: 7,
            generation: 1,
            query: "foo".into(),
            scope: scope.clone(),
            object_scope: CatalogSearchObjectScope::RelationsOnly,
            limit: 1,
        })
        .await
        .unwrap();
    assert_eq!(search.hits.len(), 1);
    assert!(search.truncated);
    assert_eq!(search.hits[0].entry.qualified_name.object, "Foo");
    let cross_profile_request = request(
        profile_id,
        CatalogTarget::objects(
            CatalogId::new(
                Uuid::new_v4(),
                CatalogKind::Schema,
                ["analytics", "analytics"],
            ),
            lazydb::db::catalog::ObjectGroup::Tables,
        )
        .unwrap(),
        CatalogScope::for_profile(DatabaseKind::ClickHouse, "analytics", None),
        1,
        None,
    );
    let error = connection
        .load_catalog_page(&cross_profile_request)
        .await
        .unwrap_err();
    assert!(error.message.contains("profile") || error.message.contains("connection"));
    connection.close().await;
    server.join().unwrap();
}

#[tokio::test]
async fn catalog_object_page_is_bounded_at_five_hundred_and_keeps_quoted_names() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        for request_index in 0..3 {
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
            let response_body = if request_index == 0 {
                "25.8.1\tanalytics\tdefault\n".to_owned()
            } else {
                assert!(
                    body.contains("LIMIT 501"),
                    "server query must be bounded: {body}"
                );
                if body.contains("lower(name) >") {
                    assert!(
                        body.contains("name > 'obj0499'"),
                        "cursor must continue after the last retained row: {body}"
                    );
                    "name\tengine\tcomment\nString\tString\tString\nobj0500\tMergeTree\t\n"
                        .to_owned()
                } else {
                    let mut payload =
                        String::from("name\tengine\tcomment\nString\tString\tString\n");
                    for index in 0..=500 {
                        let name = if index == 0 {
                            "`odd``name`".to_owned()
                        } else {
                            format!("obj{index:04}")
                        };
                        payload.push_str(&format!("{name}\tMergeTree\t\n"));
                    }
                    payload
                }
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            )
            .unwrap();
        }
    });
    let profile_id = Uuid::new_v4();
    let imported = import_connection_url(
        &format!("clickhouse://127.0.0.1:{}/analytics", address.port()),
        Some("catalog-large"),
    )
    .unwrap();
    let mut profile = imported.profile;
    profile.id = profile_id;
    let connection = DatabaseConnection::connect(&profile, None).await.unwrap();
    let schema = CatalogId::new(profile_id, CatalogKind::Schema, ["analytics", "analytics"]);
    let first = connection
        .load_catalog_page(&request(
            profile_id,
            CatalogTarget::objects(schema.clone(), lazydb::db::catalog::ObjectGroup::Tables)
                .unwrap(),
            CatalogScope::for_profile(DatabaseKind::ClickHouse, "analytics", None),
            500,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(first.entries.len(), 500);
    assert_eq!(first.entries[0].qualified_name.object, "`odd``name`");
    assert_eq!(first.entries[499].qualified_name.object, "obj0499");
    let second = connection
        .load_catalog_page(&request(
            profile_id,
            CatalogTarget::objects(schema, lazydb::db::catalog::ObjectGroup::Tables).unwrap(),
            CatalogScope::for_profile(DatabaseKind::ClickHouse, "analytics", None),
            500,
            first.next_cursor,
        ))
        .await
        .unwrap();
    assert_eq!(second.entries.len(), 1);
    assert_eq!(second.entries[0].qualified_name.object, "obj0500");
    connection.close().await;
    server.join().unwrap();
}
