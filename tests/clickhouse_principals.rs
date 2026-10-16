use lazydb::{
    db::{clickhouse::ClickHouseAdapter, principal::PrincipalCapability},
    profile::import_connection_url,
};

#[test]
fn clickhouse_principals_are_visible_read_only_and_do_not_advertise_mutation() {
    let imported =
        import_connection_url("clickhouse://localhost/analytics", Some("ch-users")).unwrap();
    let _profile_id = imported.profile.id;
    assert_eq!(
        ClickHouseAdapter::principal_capability(),
        PrincipalCapability::Details
    );
    let principal_mutations = ClickHouseAdapter::principal_mutation_capabilities();
    assert!(principal_mutations.permission_targets.is_empty());
    assert!(principal_mutations.privileges.is_empty());
    assert!(!principal_mutations.memberships);
}
