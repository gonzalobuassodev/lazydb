use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};

use lazydb::{
    db::{
        DatabaseConnection, DatabaseError, ErrorCategory,
        clickhouse::ClickHouseAdapter,
        monitor::{MAX_PROCESS_ROWS, MonitorVisibility},
    },
    model::dashboard::{MetricKey, process_matches},
    profile::import_connection_url,
};

#[allow(dead_code)]
#[path = "support/mod.rs"]
mod support;

fn fixture(responses: Vec<(u16, String)>) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(false).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let mut queries = Vec::new();
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 8192];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
                let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(str::to_owned)
                    })
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0);
                if request.len() >= header_end + 4 + content_length {
                    let sql = String::from_utf8_lossy(
                        &request[header_end + 4..header_end + 4 + content_length],
                    );
                    queries.push(sql.into_owned());
                    break;
                }
            }
            let reason = if status == 200 { "OK" } else { "Forbidden" };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
        queries
    });
    (format!("clickhouse://{address}/analytics"), handle)
}

async fn connect(url: &str) -> Result<ClickHouseAdapter, DatabaseError> {
    let imported = import_connection_url(url, Some("monitor-mock")).unwrap();
    ClickHouseAdapter::connect(&imported.profile, imported.transient_password.as_ref()).await
}

fn probe() -> (u16, String) {
    (200, "version()\tcurrentDatabase()\tcurrentUser()\nString\tString\tString\n25.8.1\tanalytics\tdefault\n".into())
}

fn event_body(rows: &str) -> String {
    format!("event\tvalue\nString\tUInt64\n{rows}")
}

fn process_header() -> String {
    "query_id\tuser\tinitial_user\taddress\tinitial_address\tcurrent_database\tquery\telapsed\tis_initial_query\nString\tString\tString\tString\tString\tString\tString\tFloat64\tUInt8\n".into()
}

#[tokio::test]
async fn clickhouse_monitor_maps_only_real_counters_and_preserves_missing_metrics() {
    let (url, server) = fixture(vec![
        probe(),
        (200, event_body("Query\t10\nSelectQuery\t7\n")),
    ]);
    let adapter = connect(&url).await.unwrap();
    let snapshot = adapter.load_monitor_snapshot().await.unwrap();
    assert_eq!(snapshot.values.get(&MetricKey::Queries), Some(&10.0));
    assert_eq!(snapshot.values.get(&MetricKey::Selects), Some(&7.0));
    assert_eq!(snapshot.values.get(&MetricKey::Inserts), None);
    assert!(!snapshot.values.contains_key(&MetricKey::Connections));
    assert!(!snapshot.values.contains_key(&MetricKey::Updates));
    assert_eq!(snapshot.visibility, MonitorVisibility::Unknown);
    assert!(
        lazydb::db::capabilities::DatabaseCapabilities::for_kind(
            lazydb::profile::DatabaseKind::ClickHouse
        )
        .monitoring
    );
    let queries = server.join().unwrap();
    assert!(queries[1].contains("FROM system.events"));
}

#[tokio::test]
async fn clickhouse_process_list_keeps_full_native_ids_long_queries_and_caps_rows() {
    let mut body = process_header();
    let long_query = format!("SELECT '{}'", "x".repeat(12_000));
    for index in 0..=MAX_PROCESS_ROWS {
        let query = if index == 0 {
            long_query.as_str()
        } else {
            "SELECT 1"
        };
        body.push_str(&format!(
            "shared-prefix-{index:04}\talice\talice\t127.0.0.1\t127.0.0.1\tanalytics\t{query}\t12.75\t1\n"
        ));
    }
    let (url, server) = fixture(vec![probe(), (200, body)]);
    let adapter = connect(&url).await.unwrap();
    let snapshot = adapter.load_process_snapshot().await.unwrap();
    assert_eq!(snapshot.rows.len(), MAX_PROCESS_ROWS);
    assert!(snapshot.truncated);
    assert!(snapshot.rows.iter().all(|row| row.id == 0));
    assert_eq!(
        snapshot.visibility,
        lazydb::db::monitor::MonitorVisibility::Unknown
    );
    assert_eq!(
        snapshot.rows[0].native_id.as_deref(),
        Some("shared-prefix-0000")
    );
    assert_ne!(snapshot.rows[0].native_id, snapshot.rows[1].native_id);
    assert!(snapshot.rows[0].elapsed.as_secs() >= 12);
    assert_eq!(snapshot.rows[0].query.as_deref(), Some(long_query.as_str()));
    assert!(process_matches(&snapshot.rows[0], "shared-prefix-0000"));
    assert!(!process_matches(&snapshot.rows[0], "shared-prefix-1999"));
    let queries = server.join().unwrap();
    assert!(queries[1].contains(&format!("LIMIT {}", MAX_PROCESS_ROWS + 1)));
    assert!(queries[1].contains("WHERE is_initial_query = 1"));
}

#[tokio::test]
async fn clickhouse_process_visibility_is_unknown_for_an_empty_visible_subset() {
    let (url, server) = fixture(vec![probe(), (200, process_header())]);
    let adapter = connect(&url).await.unwrap();
    let snapshot = adapter.load_process_snapshot().await.unwrap();
    assert!(snapshot.rows.is_empty());
    assert_eq!(
        snapshot.visibility,
        lazydb::db::monitor::MonitorVisibility::Unknown
    );
    let _ = server.join().unwrap();
}

#[tokio::test]
async fn clickhouse_monitor_permission_denial_is_not_an_empty_snapshot() {
    let (url, server) = fixture(vec![
        probe(),
        (403, "ACCESS_DENIED: system.processes".into()),
    ]);
    let adapter = connect(&url).await.unwrap();
    let error = adapter.load_process_snapshot().await.unwrap_err();
    assert_eq!(error.category, ErrorCategory::Permission, "{error:?}");
    assert!(error.message.contains("visibility is restricted"));
    assert_eq!(error.code.as_deref(), Some("clickhouse_monitor_restricted"));
    assert!(
        lazydb::db::capabilities::DatabaseCapabilities::for_kind(
            lazydb::profile::DatabaseKind::ClickHouse
        )
        .monitoring
    );
    let _ = server.join().unwrap();
}

#[tokio::test]
async fn clickhouse_monitor_reports_missing_event_set_as_missing_values() {
    let (url, server) = fixture(vec![probe(), (200, event_body(""))]);
    let adapter = connect(&url).await.unwrap();
    let snapshot = adapter.load_monitor_snapshot().await.unwrap();
    assert!(snapshot.values.is_empty());
    let _ = server.join().unwrap();
}

#[tokio::test]
async fn clickhouse_live_monitoring_reads_supported_values_and_bounded_processes() {
    let Some(url) = support::clickhouse_test_url() else {
        return;
    };
    let imported = import_connection_url(&url, Some("clickhouse-monitor-live")).unwrap();
    let connection =
        DatabaseConnection::connect(&imported.profile, imported.transient_password.as_ref())
            .await
            .expect("connect to ClickHouse fixture");
    let metrics = connection
        .load_monitor_snapshot()
        .await
        .expect("read ClickHouse event counters");
    assert!(metrics.values.keys().all(|metric| matches!(
        metric,
        MetricKey::Queries | MetricKey::Selects | MetricKey::Inserts
    )));
    let processes = connection
        .load_process_snapshot()
        .await
        .expect("read ClickHouse process snapshot");
    assert!(processes.rows.len() <= MAX_PROCESS_ROWS);
    assert_eq!(processes.visibility, MonitorVisibility::Unknown);
    let capabilities = connection.capabilities();
    assert!(capabilities.monitoring);
    assert!(!capabilities.cancellation);
    connection.close().await;
}

#[tokio::test]
async fn clickhouse_live_restricted_account_never_claims_full_visibility() {
    let Some(url) = support::clickhouse_test_url() else {
        return;
    };
    let imported = import_connection_url(&url, Some("clickhouse-monitor-live")).unwrap();
    let mut profile = imported.profile;
    profile.id = uuid::Uuid::new_v4();
    profile.user = Some("lazydb_restricted".into());
    let adapter = ClickHouseAdapter::connect(&profile, None)
        .await
        .expect("restricted user should pass the identity probe");
    match adapter.load_process_snapshot().await {
        Ok(snapshot) => assert_eq!(snapshot.visibility, MonitorVisibility::Unknown),
        Err(error) => {
            assert_eq!(error.category, ErrorCategory::Permission, "{error:?}");
            assert_eq!(error.code.as_deref(), Some("clickhouse_monitor_restricted"));
        }
    }
    adapter.close().await;
}
