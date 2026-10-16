use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
    thread,
};

use async_trait::async_trait;
use lazydb::{
    agent::{
        context::AgentProjectContext,
        policy::{PolicyError, WritePolicy, authorize_query, authorize_write},
        selection::AgentErrorCode,
        service::AgentService,
    },
    db::catalog::{
        CatalogEntry, CatalogId, CatalogKind, CatalogMetadata, ColumnMetadata, OptionalMetadata,
        QualifiedName,
    },
    lsp::{completion::complete_document, document::Document},
    persistence::{
        credentials::CredentialResolver,
        local_credentials::LocalCredentialStore,
        secrets::{SecretStore, SecretStoreError},
    },
    profile::{CredentialPolicy, Environment, ProfileAccess, import_connection_url},
    sql::{CompletionIndex, SqlDialect},
};
use secrecy::SecretString;
use tempfile::TempDir;
use tower_lsp_server::ls_types::{CompletionResponse, Position, Uri};
use uuid::Uuid;

struct EmptySecrets;

#[async_trait]
impl SecretStore for EmptySecrets {
    async fn available(&self) -> Result<(), SecretStoreError> {
        Ok(())
    }
    async fn get(&self, _: Uuid) -> Result<Option<SecretString>, SecretStoreError> {
        Ok(None)
    }
    async fn set(&self, _: Uuid, _: &SecretString) -> Result<(), SecretStoreError> {
        Ok(())
    }
    async fn delete(&self, _: Uuid) -> Result<(), SecretStoreError> {
        Ok(())
    }
}

fn service(temp: &TempDir, profiles: Vec<lazydb::profile::ConnectionProfile>) -> AgentService {
    AgentService::new(
        AgentProjectContext::resolve(Some(temp.path())).unwrap(),
        profiles,
        CredentialResolver::new(
            Arc::new(EmptySecrets),
            LocalCredentialStore::new(temp.path().join("credential.key"), "test"),
        ),
    )
    .with_limits(1, 1024 * 1024)
}

fn respond(stream: &mut std::net::TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).unwrap();
}

fn clickhouse_fixture() -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        // Probe, bounded user query, then schema search.
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 2048];
            loop {
                let count = stream.read(&mut chunk).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..count]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    // The fixture statements are small; read the request body after headers.
                    let header_end = request
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .unwrap()
                        + 4;
                    let headers =
                        String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
                    let content_length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= header_end + content_length {
                        break;
                    }
                }
            }
            let raw = String::from_utf8_lossy(&request).to_string();
            let header_end = raw.find("\r\n\r\n").unwrap_or(raw.len()) + 4;
            let sql = raw[header_end..].trim().to_owned();
            requests.push(sql.clone());
            if sql.contains("SELECT version()") {
                respond(
                    &mut stream,
                    "200 OK",
                    "version()\tcurrentDatabase()\tcurrentUser()\nString\tString\tString\n25.8.1\tanalytics\tdefault\n",
                );
            } else if sql.contains("FROM system.tables UNION ALL") {
                respond(
                    &mut stream,
                    "200 OK",
                    "database\tname\tnative_kind\tcomment\nString\tString\tString\tString\nanalytics\tevents\tMergeTree\timportant\n",
                );
            } else {
                respond(&mut stream, "200 OK", "n\nUInt64\n1\n2\n");
            }
        }
        requests
    });
    (format!("clickhouse://{address}/analytics"), server)
}

#[tokio::test]
async fn clickhouse_agent_selects_visible_profile_queries_with_budget_and_searches_scoped_catalog()
{
    let temp = TempDir::new().unwrap();
    fs::create_dir(temp.path().join(".git")).unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (url, server) = clickhouse_fixture();
    let mut selected = import_connection_url(&url, Some("warehouse-ch"))
        .unwrap()
        .profile;
    selected.access = ProfileAccess::Projects {
        roots: vec![root.clone()],
    };
    selected.catalog_scope = lazydb::profile::CatalogScope::for_profile(
        selected.kind,
        "analytics",
        selected.default_schema.as_deref(),
    );
    selected.credential_policy = CredentialPolicy::None;
    let mut hidden = import_connection_url("clickhouse://127.0.0.1:1/other", Some("hidden-ch"))
        .unwrap()
        .profile;
    hidden.access = ProfileAccess::Projects {
        roots: vec![temp.path().join("another-project")],
    };
    let agent = service(&temp, vec![selected, hidden]);

    assert_eq!(agent.connections().len(), 1);
    assert_eq!(agent.connections()[0].name, "warehouse-ch");
    assert_eq!(
        agent.select(Some("hidden-ch")).unwrap_err().code,
        AgentErrorCode::ConnectionNotFound
    );

    let query = agent
        .query(Some("warehouse-ch"), "SELECT number AS n FROM numbers(2)")
        .await
        .unwrap();
    assert_eq!(
        query.target.connection.kind,
        lazydb::profile::DatabaseKind::ClickHouse
    );
    assert_eq!(query.outcome.row_count, 1);
    assert!(query.outcome.truncated);
    assert_eq!(query.outcome.result_sets[0].columns[0].type_name, "UInt64");

    let search = agent
        .search_schema(Some("warehouse-ch"), "event".into(), 5)
        .await
        .unwrap();
    assert_eq!(
        search.target.connection.database.as_deref(),
        Some("analytics")
    );
    assert_eq!(search.hits[0].name, "events");
    assert_eq!(search.hits[0].qualified_path, "analytics.events");
    assert!(!search.truncated);

    let requests = server.join().unwrap();
    assert!(requests[0].contains("currentDatabase()"));
    assert!(requests[1].contains("FORMAT TabSeparatedWithNamesAndTypes"));
    assert!(requests[3].contains("system.tables UNION ALL"));
    assert!(requests[3].contains("database = 'analytics'"));
}

#[test]
fn clickhouse_agent_policy_maps_dialect_and_fails_closed_for_native_admin_sql() {
    let mut profile = import_connection_url("clickhouse://localhost/analytics", Some("ch"))
        .unwrap()
        .profile;
    assert_eq!(
        SqlDialect::for_database_kind(profile.kind),
        SqlDialect::ClickHouse
    );
    assert_eq!(
        lazydb::cli::LspDialect::from(profile.kind),
        lazydb::cli::LspDialect::ClickHouse
    );
    assert_eq!(authorize_query(&profile, "SELECT 1"), Ok(()));
    for sql in [
        "SYSTEM DROP DNS CACHE",
        "KILL QUERY WHERE query_id = 'q1'",
        "ALTER TABLE events DELETE WHERE id = 1",
    ] {
        assert_eq!(
            authorize_query(&profile, sql),
            Err(PolicyError::ReadOnlyQueryRequired),
            "{sql}"
        );
        assert_eq!(
            authorize_write(&profile, WritePolicy::All, sql),
            Err(PolicyError::UnknownSql),
            "{sql}"
        );
    }

    assert_eq!(
        authorize_write(&profile, WritePolicy::Deny, "INSERT INTO events VALUES (1)"),
        Err(PolicyError::ServerWritePolicyDenied)
    );
    profile.environment = Environment::Production;
    assert_eq!(
        authorize_write(
            &profile,
            WritePolicy::NonProduction,
            "INSERT INTO events VALUES (1)"
        ),
        Err(PolicyError::ProductionWriteDisabled)
    );
    assert_eq!(
        authorize_write(&profile, WritePolicy::All, "INSERT INTO events VALUES (1)"),
        Ok(())
    );
    profile.read_only = true;
    assert_eq!(
        authorize_write(&profile, WritePolicy::All, "INSERT INTO events VALUES (1)"),
        Err(PolicyError::ProfileReadOnly)
    );
}

#[test]
fn clickhouse_lsp_offline_completion_preserves_database_and_table_names() {
    let connection_id = Uuid::new_v4();
    let database_id = CatalogId::new(connection_id, CatalogKind::Database, ["analytics"]);
    let schema_id = CatalogId::new(
        connection_id,
        CatalogKind::Schema,
        ["analytics", "analytics"],
    );
    let table_id = CatalogId::new(
        connection_id,
        CatalogKind::Table,
        ["analytics", "analytics", "events"],
    );
    let database = CatalogEntry::database(
        database_id.clone(),
        QualifiedName {
            database: Some("analytics".into()),
            schema: None,
            object: "analytics".into(),
        },
        "DATABASE",
        OptionalMetadata::Unsupported,
        true,
    )
    .unwrap();
    let schema = CatalogEntry::schema(
        schema_id.clone(),
        database_id,
        QualifiedName {
            database: Some("analytics".into()),
            schema: Some("analytics".into()),
            object: "analytics".into(),
        },
        "DATABASE_NAMESPACE",
        OptionalMetadata::Unsupported,
        true,
    )
    .unwrap();
    let table = CatalogEntry::relation(
        table_id,
        schema_id,
        QualifiedName {
            database: Some("analytics".into()),
            schema: Some("analytics".into()),
            object: "events".into(),
        },
        "MergeTree",
        OptionalMetadata::Supported(None),
        true,
    )
    .unwrap();
    let entries = vec![database, schema, table];
    let index = CompletionIndex::new(&entries);
    let text = "SELECT * FROM eve";
    let document = Document {
        uri: "file:///tmp/clickhouse.sql".parse::<Uri>().unwrap(),
        language_id: "sql".into(),
        version: 1,
        text: text.into(),
    };
    let CompletionResponse::List(list) = complete_document(
        &document,
        Position::new(0, text.len() as u32),
        SqlDialect::ClickHouse,
        &index,
        false,
    ) else {
        panic!("expected completion list")
    };
    let event = list
        .items
        .iter()
        .find(|item| item.label == "events")
        .unwrap();
    let text_edit = serde_json::to_value(event.text_edit.as_ref().unwrap()).unwrap();
    assert_eq!(text_edit["newText"], "events");
    assert!(
        entries
            .iter()
            .any(|entry| entry.qualified_name.object == "analytics")
    );
    let _ = (
        CatalogMetadata::Column(ColumnMetadata::new(1, "UInt64", false)),
        list,
    );
}
