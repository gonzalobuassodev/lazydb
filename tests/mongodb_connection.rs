mod support;

use lazydb::{db::DatabaseConnection, profile::import_connection_url};

#[tokio::test]
async fn mongodb_ping_and_probe_work_with_isolated_test_deployment() {
    let Some(url) = support::mongodb_test_url() else {
        return;
    };
    let imported = import_connection_url(&url, Some("mongodb-test")).unwrap();
    let connection =
        DatabaseConnection::connect(&imported.profile, imported.transient_password.as_ref())
            .await
            .unwrap();
    assert_eq!(connection.kind(), lazydb::profile::DatabaseKind::MongoDb);
    let info = connection.probe().await.unwrap();
    assert_eq!(info.kind, lazydb::profile::DatabaseKind::MongoDb);
    assert!(!info.version.is_empty());
    connection.close().await;
}

#[tokio::test]
async fn mongodb_authentication_failure_does_not_echo_uri_credentials() {
    let Some(url) = support::mongodb_test_url() else {
        return;
    };
    let bad_url = url.replace("mongodb://", "mongodb://wrong-user:wrong-pass@");
    let imported = import_connection_url(&bad_url, Some("mongodb-bad-auth")).unwrap();
    let error =
        DatabaseConnection::connect(&imported.profile, imported.transient_password.as_ref())
            .await
            .unwrap_err();
    let output = format!("{error}");
    assert!(!output.contains("wrong-pass"));
    assert!(!output.contains("wrong-user"));
}

#[tokio::test]
async fn mongodb_standalone_rejects_manual_transaction_topology() {
    let Some(url) = support::mongodb_standalone_url() else {
        return;
    };
    let imported = import_connection_url(&url, Some("mongodb-standalone-test")).unwrap();
    let connection =
        DatabaseConnection::connect(&imported.profile, imported.transient_password.as_ref())
            .await
            .unwrap();
    let DatabaseConnection::MongoDb(adapter) = connection else {
        panic!("expected MongoDB adapter");
    };
    let error = match adapter.begin_session_transaction().await {
        Ok(_) => panic!("standalone MongoDB must reject manual transactions"),
        Err(error) => error,
    };
    assert!(error.message.contains("replica set or sharded cluster"));
    adapter.close().await;
}
