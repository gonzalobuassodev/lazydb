use lazydb::{
    db::capabilities::{InteractionModel, interaction_model},
    model::execution_target::ExecutionTarget,
    profile::{
        ConnectionUrlFormat, DatabaseKind, ProfileError, SslMode, format_connection_url,
        import_connection_url, parse_connection_url,
    },
    sql::SqlDialect,
};
use secrecy::ExposeSecret;

#[test]
fn valkey_is_a_key_value_database_with_stable_defaults() {
    let parsed = parse_connection_url("valkey://localhost").unwrap();
    assert_eq!(parsed.kind, DatabaseKind::Valkey);
    assert_eq!(parsed.port, Some(6379));
    assert_eq!(parsed.database.as_deref(), Some("0"));
    assert_eq!(parsed.ssl_mode, SslMode::Disable);
    assert_eq!(interaction_model(parsed.kind), InteractionModel::KeyValue);
    assert_eq!(SqlDialect::try_for_database_kind(parsed.kind), None);

    let profile = import_connection_url("valkey://localhost", Some("cache"))
        .unwrap()
        .profile;
    let target = ExecutionTarget::from_profile(&profile);
    assert_eq!(target.database, "0");
    assert_eq!(target.schema, None);
    assert!(target.is_valid(&profile));
    let mut invalid = target;
    invalid.schema = Some("public".into());
    assert!(!invalid.is_valid(&profile));
}

#[test]
fn valkey_url_preserves_acl_tls_read_only_and_hides_password() {
    let parsed =
        parse_connection_url("valkeys://acl%20user:p%40ss@valkey.example:6380/0002?readOnly=true")
            .unwrap();
    assert_eq!(parsed.kind, DatabaseKind::Valkey);
    assert_eq!(parsed.format, ConnectionUrlFormat::ValkeyTls);
    assert_eq!(parsed.user.as_deref(), Some("acl user"));
    assert_eq!(parsed.password.as_ref().unwrap().expose_secret(), "p@ss");
    assert_eq!(parsed.database.as_deref(), Some("2"));
    assert_eq!(parsed.ssl_mode, SslMode::Require);
    assert!(parsed.read_only);

    let imported = import_connection_url(
        "valkey://acl-user:secret@localhost/0?readOnly=true",
        Some("cache"),
    )
    .unwrap();
    let formatted = format_connection_url(&imported.profile, ConnectionUrlFormat::Valkey).unwrap();
    assert_eq!(
        formatted,
        "valkey://acl-user@localhost:6379/0?readOnly=true"
    );
    assert!(!formatted.contains("secret"));
}

#[test]
fn valkey_and_redis_schemes_keep_distinct_profile_identity() {
    for (url, kind, format) in [
        (
            "redis://localhost",
            DatabaseKind::Redis,
            ConnectionUrlFormat::Redis,
        ),
        (
            "rediss://localhost",
            DatabaseKind::Redis,
            ConnectionUrlFormat::RedisTls,
        ),
        (
            "valkey://localhost",
            DatabaseKind::Valkey,
            ConnectionUrlFormat::Valkey,
        ),
        (
            "valkeys://localhost",
            DatabaseKind::Valkey,
            ConnectionUrlFormat::ValkeyTls,
        ),
    ] {
        let parsed = parse_connection_url(url).unwrap();
        assert_eq!(parsed.kind, kind, "{url}");
        assert_eq!(parsed.format, format, "{url}");
        assert!(format.is_compatible(kind));
    }
    assert!(!ConnectionUrlFormat::Redis.is_compatible(DatabaseKind::Valkey));
    assert!(!ConnectionUrlFormat::Valkey.is_compatible(DatabaseKind::Redis));
}

#[test]
fn valkey_urls_reject_invalid_databases_and_unknown_options() {
    for url in [
        "valkey://localhost/-1",
        "valkey://localhost/not-a-db",
        "valkey://localhost/4294967296",
        "valkey://localhost/0?sslmode=require",
    ] {
        assert!(
            matches!(
                parse_connection_url(url),
                Err(ProfileError::InvalidQueryParameter(_))
                    | Err(ProfileError::UnknownQueryParameter(_))
            ),
            "accepted {url}"
        );
    }
}
