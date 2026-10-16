use std::sync::Arc;

use async_trait::async_trait;
use lazydb::{
    agent::{context::AgentProjectContext, selection::AgentErrorCode, service::AgentService},
    persistence::{
        credentials::CredentialResolver,
        local_credentials::LocalCredentialStore,
        secrets::{SecretStore, SecretStoreError},
    },
    profile::{Environment, import_connection_url},
};
use secrecy::SecretString;
use tempfile::TempDir;
use uuid::Uuid;

#[derive(Default)]
struct EmptySecrets;

#[async_trait]
impl SecretStore for EmptySecrets {
    async fn available(&self) -> Result<(), SecretStoreError> {
        Ok(())
    }
    async fn get(&self, _id: Uuid) -> Result<Option<SecretString>, SecretStoreError> {
        Ok(None)
    }
    async fn set(&self, _id: Uuid, _value: &SecretString) -> Result<(), SecretStoreError> {
        Ok(())
    }
    async fn delete(&self, _id: Uuid) -> Result<(), SecretStoreError> {
        Ok(())
    }
}

fn service(temp: &TempDir) -> AgentService {
    let profile = import_connection_url("mongodb://localhost/app", Some("mongo"))
        .unwrap()
        .profile;
    let context = AgentProjectContext::resolve(Some(temp.path())).unwrap();
    let resolver = CredentialResolver::new(
        Arc::new(EmptySecrets),
        LocalCredentialStore::new(temp.path().join("key"), "test"),
    );
    AgentService::new(context, vec![profile], resolver)
}

fn service_with_profile(temp: &TempDir, read_only: bool, environment: Environment) -> AgentService {
    let mut profile = import_connection_url("mongodb://localhost/app", Some("mongo"))
        .unwrap()
        .profile;
    profile.read_only = read_only;
    profile.environment = environment;
    let context = AgentProjectContext::resolve(Some(temp.path())).unwrap();
    let resolver = CredentialResolver::new(
        Arc::new(EmptySecrets),
        LocalCredentialStore::new(temp.path().join("key"), "test"),
    );
    AgentService::new(context, vec![profile], resolver)
}

#[tokio::test]
async fn mongo_agent_rejects_unsupported_or_write_commands_before_network_io() {
    let temp = TempDir::new().unwrap();
    let service = service(&temp);
    for json in [
        r#"{"find":"items","exhaust":true}"#,
        r#"{"insert":"items","documents":[{"x":1}]}"#,
    ] {
        let error = service.mongo_query(None, "app", json).await.unwrap_err();
        assert_eq!(error.code, AgentErrorCode::PolicyDenied);
    }
}

#[tokio::test]
async fn mongo_agent_rejects_database_outside_profile_scope_before_network_io() {
    let temp = TempDir::new().unwrap();
    let service = service(&temp);
    let error = service
        .mongo_query(None, "other", r#"{"find":"items"}"#)
        .await
        .unwrap_err();
    assert_eq!(error.code, AgentErrorCode::PolicyDenied);
}

#[tokio::test]
async fn mongo_catalog_tools_reject_out_of_scope_databases_before_network_io() {
    let temp = TempDir::new().unwrap();
    let service = service(&temp);
    let list = service
        .mongo_list_collections(None, "other")
        .await
        .unwrap_err();
    assert_eq!(list.code, AgentErrorCode::PolicyDenied);
    let describe = service
        .mongo_describe_collection(None, "other", "items")
        .await
        .unwrap_err();
    assert_eq!(describe.code, AgentErrorCode::PolicyDenied);
}

#[tokio::test]
async fn mongo_execute_change_denies_by_default_read_only_and_production_before_network_io() {
    let temp = TempDir::new().unwrap();
    let command = r#"{"insert":"items","documents":[{"x":1}]}"#;
    let denied = service(&temp)
        .mongo_execute_change(
            None,
            "app",
            command,
            lazydb::agent::policy::WritePolicy::Deny,
        )
        .await
        .unwrap_err();
    assert_eq!(denied.code, AgentErrorCode::PolicyDenied);

    let read_only = service_with_profile(&temp, true, Environment::Development)
        .mongo_execute_change(
            None,
            "app",
            command,
            lazydb::agent::policy::WritePolicy::All,
        )
        .await
        .unwrap_err();
    assert_eq!(read_only.code, AgentErrorCode::PolicyDenied);

    let production = service_with_profile(&temp, false, Environment::Production)
        .mongo_execute_change(
            None,
            "app",
            command,
            lazydb::agent::policy::WritePolicy::NonProduction,
        )
        .await
        .unwrap_err();
    assert_eq!(production.code, AgentErrorCode::PolicyDenied);

    let out_of_scope = service(&temp)
        .mongo_execute_change(
            None,
            "other",
            command,
            lazydb::agent::policy::WritePolicy::All,
        )
        .await
        .unwrap_err();
    assert_eq!(out_of_scope.code, AgentErrorCode::PolicyDenied);
}

#[tokio::test]
async fn mongo_execute_change_rejects_unsupported_and_multi_operation_commands_before_network_io() {
    let temp = TempDir::new().unwrap();
    let service = service(&temp);
    for command in [
        r#"{"find":"items"}"#,
        r#"{"drop":"items"}"#,
        r#"{"insert":"items","documents":[{"x":1},{"x":2}]}"#,
        r#"{"update":"items","updates":[{"q":{},"u":{"$set":{"x":1}},"upsert":true}]}"#,
        r#"{"delete":"items","deletes":[{"q":{},"limit":0}]}"#,
        r#"{"delete":"items","deletes":[{"q":{},"limit":1}]}"#,
    ] {
        let error = service
            .mongo_execute_change(
                None,
                "app",
                command,
                lazydb::agent::policy::WritePolicy::All,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, AgentErrorCode::PolicyDenied, "{command}");
    }
}
