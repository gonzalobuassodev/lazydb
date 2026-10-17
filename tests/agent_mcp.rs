use async_trait::async_trait;
use lazydb::agent::mcp::AgentMcpServer;
use lazydb::agent::policy::WritePolicy;
use lazydb::agent::{context::AgentProjectContext, service::AgentService};
use lazydb::persistence::{
    credentials::CredentialResolver,
    local_credentials::LocalCredentialStore,
    secrets::{SecretStore, SecretStoreError},
};
use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientInfo, Implementation};
use rmcp::{ClientLifecycleMode, ClientServiceExt, ServiceExt};
use secrecy::SecretString;
use std::sync::Arc;
use tempfile::TempDir;
use uuid::Uuid;

fn sqlite_profile(
    name: &str,
    read_only: bool,
    environment: lazydb::profile::Environment,
) -> lazydb::profile::ConnectionProfile {
    let mut profile = lazydb::profile::import_connection_url(":memory:", Some(name))
        .unwrap()
        .profile;
    profile.read_only = read_only;
    profile.environment = environment;
    profile
}

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

#[test]
fn tool_definitions_are_split_between_read_and_write_operations() {
    let temp = TempDir::new().unwrap();
    std::fs::create_dir(temp.path().join(".git")).unwrap();
    let service = AgentService::new(
        AgentProjectContext::resolve(Some(temp.path())).unwrap(),
        Vec::new(),
        CredentialResolver::new(
            Arc::new(EmptySecrets),
            LocalCredentialStore::new(temp.path().join("key"), "test"),
        ),
    );
    let _server = AgentMcpServer::new(service, WritePolicy::Deny);
    assert_eq!(AgentMcpServer::get_context_tool_attr().name, "get_context");
    assert_eq!(
        AgentMcpServer::list_connections_tool_attr().name,
        "list_connections"
    );
    assert_eq!(
        AgentMcpServer::search_schema_tool_attr().name,
        "search_schema"
    );
    assert_eq!(
        AgentMcpServer::describe_object_tool_attr().name,
        "describe_object"
    );
    assert_eq!(AgentMcpServer::query_tool_attr().name, "query");
    assert_eq!(
        AgentMcpServer::execute_change_tool_attr().name,
        "execute_change"
    );
    assert_eq!(
        AgentMcpServer::execute_file_tool_attr().name,
        "execute_file"
    );
    assert_eq!(
        AgentMcpServer::query_tool_attr()
            .annotations
            .unwrap()
            .read_only_hint,
        Some(true)
    );
    assert_eq!(
        AgentMcpServer::execute_change_tool_attr()
            .annotations
            .unwrap()
            .destructive_hint,
        Some(true)
    );
    let change_tool = AgentMcpServer::execute_change_tool_attr();
    let change_description = change_tool.description.as_deref().unwrap();
    assert!(change_description.contains("Client approval does not override"));
    assert!(change_description.contains("--write-policy"));
    assert!(change_description.contains("read-only"));
    let file_tool = AgentMcpServer::execute_file_tool_attr();
    let file_description = file_tool.description.as_deref().unwrap();
    assert!(file_description.contains("Client approval does not override"));
}

#[test]
fn context_reports_server_policy_and_effective_write_capability() {
    let temp = TempDir::new().unwrap();
    std::fs::create_dir(temp.path().join(".git")).unwrap();
    let service = AgentService::new(
        AgentProjectContext::resolve(Some(temp.path())).unwrap(),
        vec![sqlite_profile(
            "app-dev",
            false,
            lazydb::profile::Environment::Development,
        )],
        CredentialResolver::new(
            Arc::new(EmptySecrets),
            LocalCredentialStore::new(temp.path().join("key"), "test"),
        ),
    );
    let server = AgentMcpServer::new(service, WritePolicy::Deny);
    let context: serde_json::Value =
        serde_json::from_str(&server.context_json(None).unwrap()).unwrap();

    assert_eq!(context["server_write_policy"], "deny");
    assert_eq!(context["write_capability"]["allowed"], false);
    assert_eq!(
        context["write_capability"]["denial_reason"],
        "server_policy"
    );
    assert!(
        context["write_capability"]["message"]
            .as_str()
            .unwrap()
            .contains("--write-policy non-production")
    );
}

#[test]
fn context_reports_allowed_and_profile_or_environment_denials() {
    let temp = TempDir::new().unwrap();
    std::fs::create_dir(temp.path().join(".git")).unwrap();
    let resolver = || {
        CredentialResolver::new(
            Arc::new(EmptySecrets),
            LocalCredentialStore::new(temp.path().join("key"), "test"),
        )
    };

    let allowed = AgentMcpServer::new(
        AgentService::new(
            AgentProjectContext::resolve(Some(temp.path())).unwrap(),
            vec![sqlite_profile(
                "app-dev",
                false,
                lazydb::profile::Environment::Development,
            )],
            resolver(),
        ),
        WritePolicy::NonProduction,
    );
    let context: serde_json::Value =
        serde_json::from_str(&allowed.context_json(None).unwrap()).unwrap();
    assert_eq!(context["server_write_policy"], "non-production");
    assert_eq!(context["write_capability"]["allowed"], true);
    assert_eq!(
        context["write_capability"]["denial_reason"],
        serde_json::Value::Null
    );
    assert_eq!(
        context["write_capability"]["message"],
        serde_json::Value::Null
    );

    let read_only = AgentMcpServer::new(
        AgentService::new(
            AgentProjectContext::resolve(Some(temp.path())).unwrap(),
            vec![sqlite_profile(
                "read-only",
                true,
                lazydb::profile::Environment::Development,
            )],
            resolver(),
        ),
        WritePolicy::All,
    );
    let context: serde_json::Value =
        serde_json::from_str(&read_only.context_json(None).unwrap()).unwrap();
    assert_eq!(
        context["write_capability"]["denial_reason"],
        "profile_read_only"
    );

    let production = AgentMcpServer::new(
        AgentService::new(
            AgentProjectContext::resolve(Some(temp.path())).unwrap(),
            vec![sqlite_profile(
                "production",
                false,
                lazydb::profile::Environment::Production,
            )],
            resolver(),
        ),
        WritePolicy::NonProduction,
    );
    let context: serde_json::Value =
        serde_json::from_str(&production.context_json(None).unwrap()).unwrap();
    assert_eq!(
        context["write_capability"]["denial_reason"],
        "production_policy"
    );
}

#[tokio::test]
async fn duckdb_query_is_available_over_mcp_wire_protocol() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    let db_path = temp.path().join("mcp.duckdb");
    let profile = lazydb::profile::import_connection_url(
        &format!("duckdb:{}", db_path.display()),
        Some("mcp-duckdb"),
    )
    .unwrap()
    .profile;
    let service = AgentService::new(
        AgentProjectContext::resolve(Some(&project)).unwrap(),
        vec![profile],
        CredentialResolver::new(
            Arc::new(EmptySecrets),
            LocalCredentialStore::new(temp.path().join("key"), "test"),
        ),
    );
    let server = AgentMcpServer::new(service, WritePolicy::Deny);
    let (server_io, client_io) = tokio::io::duplex(16 * 1024);
    let (client_read, client_write) = tokio::io::split(client_io);
    let server_task = tokio::spawn(server.serve(server_io));
    let client: ClientInfo = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("lazydb-mcp-test", "0.1.0"),
    );
    let mut client = client
        .serve_with_lifecycle((client_read, client_write), ClientLifecycleMode::Initialize)
        .await
        .unwrap();
    let tools = client.list_tools(None).await.unwrap();
    assert!(tools.tools.iter().any(|tool| tool.name == "query"));
    let result = client
        .call_tool(
            CallToolRequestParams::new("query").with_arguments(
                serde_json::json!({"connection":"mcp-duckdb", "sql":"SELECT 42 AS answer"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert!(!result.is_error.unwrap_or(true));
    let text = result
        .content
        .iter()
        .find_map(|content| content.as_text().map(|text| text.text.as_str()));
    assert!(
        text.is_some_and(|text| text.contains("42")),
        "MCP content: {text:?}"
    );
    client.close().await.unwrap();
    drop(client);
    let _server = server_task.await.unwrap().unwrap();
}

#[tokio::test]
async fn duckdb_mcp_write_policy_denies_known_and_unknown_side_effects_without_changes() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    let db_path = temp.path().join("mcp-policy.duckdb");
    let profile = lazydb::profile::import_connection_url(
        &format!("duckdb:{}", db_path.display()),
        Some("mcp-duckdb-policy"),
    )
    .unwrap()
    .profile;
    let setup = lazydb::db::DatabaseConnection::connect(&profile, None)
        .await
        .unwrap();
    setup
        .execute("CREATE TABLE policy_rows (id INTEGER); INSERT INTO policy_rows VALUES (1)")
        .await
        .unwrap();
    setup.close().await;

    let service = AgentService::new(
        AgentProjectContext::resolve(Some(&project)).unwrap(),
        vec![profile.clone()],
        CredentialResolver::new(
            Arc::new(EmptySecrets),
            LocalCredentialStore::new(temp.path().join("key-policy"), "test"),
        ),
    );
    let server = AgentMcpServer::new(service, WritePolicy::Deny);
    let (server_io, client_io) = tokio::io::duplex(16 * 1024);
    let (client_read, client_write) = tokio::io::split(client_io);
    let server_task = tokio::spawn(server.serve(server_io));
    let client_info = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("lazydb-mcp-policy-test", "0.1.0"),
    );
    let mut client = client_info
        .serve_with_lifecycle((client_read, client_write), ClientLifecycleMode::Initialize)
        .await
        .unwrap();

    let copy_path = temp.path().join("denied-copy.csv");
    let sql_statements = [
        "INSERT INTO policy_rows VALUES (2)".to_owned(),
        format!("COPY policy_rows TO '{}' (HEADER)", copy_path.display()),
        "ATTACH ':memory:' AS denied_catalog".to_owned(),
        "INSTALL httpfs".to_owned(),
        "LOAD httpfs".to_owned(),
        "EXPLAIN ANALYZE INSERT INTO policy_rows VALUES (3)".to_owned(),
        "SELECT nextval('missing_sequence')".to_owned(),
        "SELECT setval('missing_sequence', 9)".to_owned(),
    ];
    for sql in &sql_statements {
        let result = client
            .call_tool(
                CallToolRequestParams::new("execute_change").with_arguments(
                    serde_json::json!({"connection":"mcp-duckdb-policy", "sql":sql})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert!(
            result.is_error.unwrap_or(false),
            "unexpected success for {sql}"
        );
    }
    assert!(!copy_path.exists(), "denied COPY created an output file");
    let attached = client
        .call_tool(
            CallToolRequestParams::new("query").with_arguments(
                serde_json::json!({"connection":"mcp-duckdb-policy", "sql":"SELECT count(*) FROM duckdb_databases() WHERE database_name = 'denied_catalog'"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert!(!attached.is_error.unwrap_or(true));
    let attached_text = attached
        .content
        .iter()
        .find_map(|content| content.as_text().map(|text| text.text.as_str()))
        .unwrap_or_default();
    assert!(
        attached_text.contains('0'),
        "catalog remains unattached: {attached_text}"
    );

    let query = client
        .call_tool(
            CallToolRequestParams::new("query").with_arguments(
                serde_json::json!({"connection":"mcp-duckdb-policy", "sql":"SELECT count(*) AS n FROM policy_rows"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert!(!query.is_error.unwrap_or(true));
    let query_text = query
        .content
        .iter()
        .find_map(|content| content.as_text().map(|text| text.text.as_str()))
        .unwrap_or_default();
    assert!(query_text.contains("1"), "MCP query result: {query_text}");

    client.close().await.unwrap();
    drop(client);
    let _server = server_task.await.unwrap().unwrap();
}
