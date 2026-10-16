use lazydb::{
    db::{
        capabilities::{InteractionModel, interaction_model},
        redis::types::{RedisTarget, ScanPosition},
    },
    profile::DatabaseKind,
    sql::SqlDialect,
};
use uuid::Uuid;

#[test]
fn redis_uses_key_value_capabilities_and_has_no_sql_dialect() {
    assert_eq!(
        interaction_model(DatabaseKind::Redis),
        InteractionModel::KeyValue
    );
    assert_eq!(
        interaction_model(DatabaseKind::Postgres),
        InteractionModel::Relational
    );
    assert_eq!(
        interaction_model(DatabaseKind::MongoDb),
        InteractionModel::Document
    );
    assert_eq!(SqlDialect::try_for_database_kind(DatabaseKind::Redis), None);
    assert_eq!(
        SqlDialect::try_for_database_kind(DatabaseKind::Postgres),
        Some(SqlDialect::Postgres)
    );
    assert_eq!(
        SqlDialect::try_for_database_kind(DatabaseKind::MongoDb),
        None
    );
}

#[test]
fn redis_target_normalizes_database_numbers_and_scan_completion() {
    let profile_id = Uuid::from_u128(1);
    assert_eq!(RedisTarget::new(profile_id, "0002").unwrap().database, 2);
    assert!(RedisTarget::new(profile_id, "-1").is_none());
    assert!(RedisTarget::new(profile_id, "not-a-db").is_none());
    assert_eq!(ScanPosition::next(0), ScanPosition::Complete);
    assert_eq!(ScanPosition::next(91), ScanPosition::Continue(91));
}

#[tokio::test]
#[ignore = "requires an isolated Redis server"]
async fn redis_adapter_connects_to_the_configured_database_and_probes_it() {
    let url = std::env::var("LAZYDB_TEST_REDIS_URL").expect("LAZYDB_TEST_REDIS_URL");
    let imported = lazydb::profile::import_connection_url(&url, Some("redis-test")).unwrap();
    let adapter = lazydb::db::redis::RedisAdapter::connect(&imported.profile, None)
        .await
        .unwrap();
    assert_eq!(
        adapter.database(),
        imported
            .profile
            .database
            .as_deref()
            .unwrap()
            .parse::<u32>()
            .unwrap()
    );
    assert_eq!(adapter.probe().await.unwrap().kind, DatabaseKind::Redis);
    adapter.close().await;
}

#[tokio::test]
#[ignore = "requires an isolated Redis server"]
async fn redis_adapter_scan_returns_native_cursor_and_keys() {
    let url = std::env::var("LAZYDB_TEST_REDIS_URL").expect("LAZYDB_TEST_REDIS_URL");
    let imported = lazydb::profile::import_connection_url(&url, Some("redis-test")).unwrap();
    let adapter = lazydb::db::redis::RedisAdapter::connect(&imported.profile, None)
        .await
        .unwrap();
    let (cursor, keys) = adapter.scan_keys(0, b"lazydb-task4-*", 20).await.unwrap();
    let _cursor = cursor;
    assert!(keys.iter().all(|key| key.starts_with(b"lazydb-task4-")));
    adapter.close().await;
}

#[tokio::test]
#[ignore = "requires an isolated Redis server"]
async fn redis_adapter_preview_reads_core_value_types() {
    let url = std::env::var("LAZYDB_TEST_REDIS_URL").expect("LAZYDB_TEST_REDIS_URL");
    let imported = lazydb::profile::import_connection_url(&url, Some("redis-test")).unwrap();
    let adapter = lazydb::db::redis::RedisAdapter::connect(&imported.profile, None)
        .await
        .unwrap();
    let mut connection = redis::Client::open(url.clone())
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    redis::cmd("SET")
        .arg("lazydb-preview-string")
        .arg("hello")
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    redis::cmd("HSET")
        .arg("lazydb-preview-hash")
        .arg("field")
        .arg("value")
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    redis::cmd("LPUSH")
        .arg("lazydb-preview-list")
        .arg("item")
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    redis::cmd("SADD")
        .arg("lazydb-preview-set")
        .arg("member")
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    redis::cmd("ZADD")
        .arg("lazydb-preview-zset")
        .arg(1)
        .arg("member")
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    for name in ["string", "hash", "list", "set", "zset"] {
        let key = format!("lazydb-preview-{name}").into_bytes();
        let key_id = lazydb::db::redis::types::RedisKeyId {
            target: lazydb::db::redis::types::RedisTarget {
                profile_id: imported.profile.id,
                database: 0,
            },
            key,
        };
        assert!(!adapter.preview_key(&key_id).await.unwrap().is_empty());
    }
    redis::cmd("DEL")
        .arg("lazydb-preview-string")
        .arg("lazydb-preview-hash")
        .arg("lazydb-preview-list")
        .arg("lazydb-preview-set")
        .arg("lazydb-preview-zset")
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
    adapter.close().await;
}
