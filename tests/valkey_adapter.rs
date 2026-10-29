use lazydb::identity::ConnectionIdentity;
use lazydb::{
    db::{
        DatabaseConnection,
        redis::{RedisAdapter, types::RedisKeyId},
    },
    profile::{DatabaseKind, import_connection_url},
};
use uuid::Uuid;

fn required_valkey_url() -> String {
    std::env::var("LAZYDB_TEST_VALKEY_URL").expect("LAZYDB_TEST_VALKEY_URL must be set")
}

fn redis_transport_url(url: &str) -> String {
    url.replacen("valkeys://", "rediss://", 1)
        .replacen("valkey://", "redis://", 1)
}

#[tokio::test]
#[ignore = "requires the isolated Valkey service from docker-compose.valkey.yml"]
async fn valkey_product_connection_probes_scans_and_round_trips_values() {
    let url = required_valkey_url();
    let imported = import_connection_url(&url, Some("valkey-test")).unwrap();
    assert_eq!(imported.profile.kind, DatabaseKind::Valkey);
    let connection =
        DatabaseConnection::connect(&imported.profile, imported.transient_password.as_ref())
            .await
            .unwrap();
    assert_eq!(connection.kind(), DatabaseKind::Valkey);
    let probe = connection.probe().await.unwrap();
    assert_eq!(probe.kind, DatabaseKind::Valkey);
    assert!(
        probe.version.starts_with("8.1."),
        "expected the pinned Valkey 8.1 image version, got {}",
        probe.version
    );

    let adapter = match &connection {
        DatabaseConnection::Redis(adapter) => adapter,
        _ => panic!("Valkey must use the shared Redis-compatible adapter"),
    };
    let profile_id = imported.profile.id;
    let target_database = adapter.database();
    let suffix = Uuid::new_v4().to_string();
    let prefix = format!("lazydb:valkey:{suffix}");
    let key = |kind: &str| format!("{prefix}:{kind}").into_bytes();
    let make_key_id = |key: Vec<u8>| RedisKeyId {
        target: lazydb::db::redis::types::RedisTarget {
            profile_id,
            database: target_database,
        },
        key,
    };
    let mut seed = redis::Client::open(redis_transport_url(&url))
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap();

    let string_key = key("string");
    let hash_key = key("hash");
    let list_key = key("list");
    let set_key = key("set");
    let sorted_set_key = key("zset");
    let stream_key = key("stream");
    redis::cmd("SET")
        .arg(&string_key)
        .arg(&[0_u8, 0xff, 1][..])
        .query_async::<()>(&mut seed)
        .await
        .unwrap();
    redis::cmd("HSET")
        .arg(&hash_key)
        .arg("field")
        .arg(&[0_u8, 0xff, 1][..])
        .query_async::<()>(&mut seed)
        .await
        .unwrap();
    redis::cmd("RPUSH")
        .arg(&list_key)
        .arg(&[0_u8, 0xff, 1][..])
        .query_async::<()>(&mut seed)
        .await
        .unwrap();
    redis::cmd("SADD")
        .arg(&set_key)
        .arg(&[0_u8, 0xff, 1][..])
        .query_async::<()>(&mut seed)
        .await
        .unwrap();
    redis::cmd("ZADD")
        .arg(&sorted_set_key)
        .arg(1)
        .arg(&[0_u8, 0xff, 1][..])
        .query_async::<()>(&mut seed)
        .await
        .unwrap();
    redis::cmd("XADD")
        .arg(&stream_key)
        .arg("*")
        .arg("field")
        .arg(&[0_u8, 0xff, 1][..])
        .query_async::<String>(&mut seed)
        .await
        .unwrap();

    let scan_pattern = format!("{prefix}:*");
    let mut cursor = 0;
    let mut scan_keys = Vec::new();
    loop {
        let (next, page_keys) = adapter
            .scan_keys(cursor, scan_pattern.as_bytes(), 100)
            .await
            .unwrap();
        scan_keys.extend(page_keys);
        if next == 0 {
            break;
        }
        cursor = next;
    }
    assert_eq!(scan_keys.len(), 6);
    use lazydb::db::redis::read::RedisType;
    use lazydb::db::redis::read::{RedisPageValue, RedisReadRequest};
    for (request, expected_type) in [
        (
            RedisReadRequest::StringRange {
                key: make_key_id(string_key.clone()),
                start: 0,
                end: 2,
            },
            RedisType::String,
        ),
        (
            RedisReadRequest::HashScan {
                key: make_key_id(hash_key.clone()),
                cursor: 0,
                count: 20,
            },
            RedisType::Hash,
        ),
        (
            RedisReadRequest::ListRange {
                key: make_key_id(list_key.clone()),
                start: 0,
                end: 20,
            },
            RedisType::List,
        ),
        (
            RedisReadRequest::SetScan {
                key: make_key_id(set_key.clone()),
                cursor: 0,
                count: 20,
            },
            RedisType::Set,
        ),
        (
            RedisReadRequest::SortedSetRange {
                key: make_key_id(sorted_set_key.clone()),
                start: 0,
                end: 20,
            },
            RedisType::SortedSet,
        ),
        (
            RedisReadRequest::StreamRange {
                key: make_key_id(stream_key.clone()),
                start: b"-".to_vec(),
                count: 20,
            },
            RedisType::Stream,
        ),
    ] {
        let page = adapter.read_value_page(&request).await.unwrap();
        assert_eq!(page.metadata.value_type, expected_type);
        let has_value = match &page.value {
            RedisPageValue::String(value) => !value.is_empty(),
            RedisPageValue::Hash(value) => !value.is_empty(),
            RedisPageValue::List(value) => !value.is_empty(),
            RedisPageValue::Set(value) => !value.is_empty(),
            RedisPageValue::SortedSet(value) => !value.is_empty(),
            RedisPageValue::Stream(value) => !value.is_empty(),
        };
        assert!(has_value, "empty value page for {expected_type:?}");
    }

    let hash_id = make_key_id(hash_key.clone());

    let request = lazydb::db::redis::mutation::RedisMutationRequest {
        connection: ConnectionIdentity {
            profile_id,
            generation: 1,
        },
        request_id: 1,
        mode: lazydb::db::redis::mutation::RedisMutationMode::Edit,
        key: hash_id.clone(),
    };
    let mutation = RedisAdapter::plan_operation(
        request,
        lazydb::db::redis::mutation::RedisMutationOperation::SetHashField {
            field: b"field".to_vec(),
            value: b"updated".to_vec(),
            expected: Some(vec![0, 0xff, 1]),
        },
        lazydb::db::redis::mutation::RedisTtlMutation::SetMillis(30_000),
        Some(lazydb::db::redis::mutation::RedisKeyBaseline {
            value_type: lazydb::db::redis::read::RedisType::Hash,
        }),
    )
    .unwrap();
    adapter.execute_mutation(&mutation).await.unwrap();
    let page = adapter
        .read_value_page(&lazydb::db::redis::read::RedisReadRequest::HashScan {
            key: hash_id.clone(),
            cursor: 0,
            count: 20,
        })
        .await
        .unwrap();
    assert!(matches!(
        page.value,
        lazydb::db::redis::read::RedisPageValue::Hash(ref entries)
            if entries.iter().any(|(field, value)| field == b"field" && value == b"updated")
    ));
    assert!(matches!(
        page.metadata.ttl,
        lazydb::db::redis::read::TtlState::ExpiresIn { millis } if millis > 0
    ));

    let conflict = RedisAdapter::plan_operation(
        lazydb::db::redis::mutation::RedisMutationRequest {
            connection: ConnectionIdentity {
                profile_id,
                generation: 1,
            },
            request_id: 2,
            mode: lazydb::db::redis::mutation::RedisMutationMode::Edit,
            key: hash_id.clone(),
        },
        lazydb::db::redis::mutation::RedisMutationOperation::SetHashField {
            field: b"field".to_vec(),
            value: b"must-not-apply".to_vec(),
            expected: Some(vec![0, 0xff, 1]),
        },
        lazydb::db::redis::mutation::RedisTtlMutation::Preserve,
        Some(lazydb::db::redis::mutation::RedisKeyBaseline {
            value_type: lazydb::db::redis::read::RedisType::Hash,
        }),
    )
    .unwrap();
    assert!(adapter.execute_mutation(&conflict).await.is_err());
    let page = adapter
        .read_value_page(&lazydb::db::redis::read::RedisReadRequest::HashScan {
            key: hash_id,
            cursor: 0,
            count: 20,
        })
        .await
        .unwrap();
    assert!(matches!(
        page.value,
        RedisPageValue::Hash(ref entries)
            if entries.iter().any(|(field, value)| field == b"field" && value == b"updated")
    ));

    let _: i64 = redis::cmd("DEL")
        .arg(&string_key)
        .arg(&hash_key)
        .arg(&list_key)
        .arg(&set_key)
        .arg(&sorted_set_key)
        .arg(&stream_key)
        .query_async(&mut seed)
        .await
        .unwrap();
    drop(seed);
    connection.close().await;
}

#[tokio::test]
#[ignore = "requires an isolated Valkey service from docker-compose.valkey.yml"]
async fn valkey_configured_database_is_used_by_the_product_connection() {
    let url = required_valkey_url();
    let mut imported = import_connection_url(&url, Some("valkey-db-test")).unwrap();
    imported.profile.database = Some("2".into());
    let mut seed = redis::Client::open(redis_transport_url(&url))
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    redis::cmd("SELECT")
        .arg(2)
        .query_async::<()>(&mut seed)
        .await
        .unwrap();
    let key_prefix = format!("lazydb:valkey:db2:{}", Uuid::new_v4());
    let key = key_prefix.clone();
    redis::cmd("SET")
        .arg(&key)
        .arg("db2")
        .query_async::<()>(&mut seed)
        .await
        .unwrap();

    let connection = DatabaseConnection::connect(&imported.profile, None)
        .await
        .unwrap();
    assert_eq!(connection.kind(), DatabaseKind::Valkey);
    assert_eq!(connection.probe().await.unwrap().database, "2");
    let adapter = match &connection {
        DatabaseConnection::Redis(adapter) => adapter,
        _ => panic!("Valkey must use the shared Redis-compatible adapter"),
    };
    let scan_pattern = format!("{key_prefix}*");
    let (_, keys) = adapter
        .scan_keys(0, scan_pattern.as_bytes(), 20)
        .await
        .unwrap();
    assert!(keys.iter().any(|found| found == key.as_bytes()));
    adapter.delete_key(key.as_bytes()).await.unwrap();
    let (_, keys_after_delete) = adapter
        .scan_keys(0, scan_pattern.as_bytes(), 20)
        .await
        .unwrap();
    assert!(
        !keys_after_delete
            .iter()
            .any(|found| found == key.as_bytes())
    );
    connection.close().await;
    drop(seed);
}

#[tokio::test]
#[ignore = "requires the isolated Valkey service from docker-compose.valkey.yml"]
async fn valkey_database_discovery_keeps_partial_results_when_config_is_forbidden() {
    let url = required_valkey_url();
    let mut imported = import_connection_url(&url, Some("valkey-acl-test")).unwrap();
    let username = format!("lazydb-{}", Uuid::new_v4());
    let password = format!("secret@{}:acl", Uuid::new_v4());
    let mut admin = redis::Client::open(redis_transport_url(&url))
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    redis::cmd("ACL")
        .arg("SETUSER")
        .arg(&username)
        .arg("on")
        .arg(format!(">{password}"))
        .arg("~*")
        .arg("+@all")
        .arg("-config")
        .query_async::<()>(&mut admin)
        .await
        .unwrap();

    imported.profile.user = Some(username.clone());
    let secret = secrecy::SecretString::from(password);
    let connection = DatabaseConnection::connect(&imported.profile, Some(&secret))
        .await
        .unwrap();
    if let DatabaseConnection::Redis(adapter) = &connection {
        adapter.ping().await.unwrap();
    }
    let discovery = connection.discover_redis_databases(0, &[]).await.unwrap();
    assert_eq!(
        discovery.completeness,
        lazydb::db::redis::discovery::RedisDiscoveryCompleteness::Partial
    );
    assert!(
        discovery
            .databases
            .iter()
            .any(|database| database.database == 0)
    );
    connection.close().await;

    redis::cmd("ACL")
        .arg("DELUSER")
        .arg(&username)
        .query_async::<()>(&mut admin)
        .await
        .unwrap();
    drop(admin);
}

#[tokio::test]
#[ignore = "requires the isolated Valkey service from docker-compose.valkey.yml"]
async fn valkey_acl_authentication_uses_encoded_profile_credentials() {
    let url = required_valkey_url();
    let mut imported = import_connection_url(&url, Some("valkey-auth-test")).unwrap();
    let username = format!("lazydb-auth-{}", Uuid::new_v4());
    let password = format!("secret@{}:acl", Uuid::new_v4());
    let mut admin = redis::Client::open(redis_transport_url(&url))
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    redis::cmd("ACL")
        .arg("SETUSER")
        .arg(&username)
        .arg("on")
        .arg(format!(">{password}"))
        .arg("~*")
        .arg("+@all")
        .query_async::<()>(&mut admin)
        .await
        .unwrap();

    imported.profile.user = Some(username.clone());
    let secret = secrecy::SecretString::from(password.clone());
    let connection = DatabaseConnection::connect(&imported.profile, Some(&secret))
        .await
        .unwrap();
    if let DatabaseConnection::Redis(adapter) = &connection {
        adapter.ping().await.unwrap();
    }
    connection.close().await;

    let wrong_password = secrecy::SecretString::from(format!("wrong-{password}"));
    assert!(
        DatabaseConnection::connect(&imported.profile, Some(&wrong_password))
            .await
            .is_err()
    );

    redis::cmd("ACL")
        .arg("DELUSER")
        .arg(&username)
        .query_async::<()>(&mut admin)
        .await
        .unwrap();
    drop(admin);
}
