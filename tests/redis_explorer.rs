use lazydb::{
    db::redis::discovery::{RedisDatabaseDiscovery, RedisDatabaseInfo, RedisDiscoveryCompleteness},
    model::explorer::{ExplorerNodeId, ExplorerTreeState, ProfileProvenance},
    profile::DatabaseKind,
};
use uuid::Uuid;

#[test]
fn redis_profile_projects_database_rows_in_numeric_order() {
    let profile_id = Uuid::from_u128(1);
    let mut explorer = ExplorerTreeState::default();
    explorer.add_profile_with_metadata(
        profile_id,
        "cache".into(),
        DatabaseKind::Redis,
        "localhost:6379".into(),
        ProfileProvenance::Saved,
    );
    let profile = explorer.profiles.get_mut(&profile_id).unwrap();
    profile.set_redis_databases(RedisDatabaseDiscovery {
        databases: vec![
            RedisDatabaseInfo {
                database: 0,
                keys: None,
                expires: None,
            },
            RedisDatabaseInfo {
                database: 2,
                keys: Some(1),
                expires: None,
            },
            RedisDatabaseInfo {
                database: 10,
                keys: Some(2),
                expires: None,
            },
        ],
        completeness: RedisDiscoveryCompleteness::Complete,
        warnings: Vec::new(),
    });
    explorer
        .expanded
        .insert(ExplorerNodeId::Profile(profile_id));
    let rows = explorer.visible();
    let databases = rows
        .into_iter()
        .filter_map(|row| match row.id {
            ExplorerNodeId::RedisDatabase { database, .. } => Some(database),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(databases, vec![0, 2, 10]);
}

#[test]
fn valkey_profile_projects_the_shared_redis_database_rows() {
    let profile_id = Uuid::from_u128(2);
    let mut explorer = ExplorerTreeState::default();
    explorer.add_profile_with_metadata(
        profile_id,
        "valkey".into(),
        DatabaseKind::Valkey,
        "localhost:6379".into(),
        ProfileProvenance::Saved,
    );
    let profile = explorer.profiles.get_mut(&profile_id).unwrap();
    profile.set_redis_databases(RedisDatabaseDiscovery {
        databases: vec![RedisDatabaseInfo {
            database: 2,
            keys: Some(1),
            expires: None,
        }],
        completeness: RedisDiscoveryCompleteness::Complete,
        warnings: Vec::new(),
    });
    explorer
        .expanded
        .insert(ExplorerNodeId::Profile(profile_id));

    assert!(explorer.visible().iter().any(|row| matches!(
        &row.id,
        ExplorerNodeId::RedisDatabase { profile_id: id, database: 2 } if *id == profile_id
    )));
}
