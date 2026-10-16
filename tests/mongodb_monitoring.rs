use lazydb::db::mongodb::monitor::{MongoMonitorError, MongoMonitorSnapshot};

#[test]
fn monitoring_snapshot_can_show_partial_permission_failures() {
    let snapshot = MongoMonitorSnapshot {
        sampled_at: std::time::SystemTime::UNIX_EPOCH,
        server_status: Some(mongodb::bson::doc! {"ok": 1}),
        database_stats: None,
        collection_stats: None,
        current_operations: Vec::new(),
        errors: vec![MongoMonitorError {
            command: "currentOp",
            message: "unauthorized".into(),
        }],
    };
    assert!(snapshot.server_status.is_some());
    assert_eq!(snapshot.errors.first().unwrap().command, "currentOp");
}
