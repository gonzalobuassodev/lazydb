use lazydb::{
    db::{
        capabilities::{DatabaseCapabilities, InteractionModel, interaction_model},
        descriptor::{DRIVERS, DatabaseCategory, drivers_in},
    },
    persistence::profiles::ProfileStore,
    profile::{ConnectionUrlFormat, DatabaseKind, format_connection_url, import_connection_url},
    sql::SqlDialect,
};
use tempfile::TempDir;

#[test]
fn mongodb_is_registered_as_a_document_driver_without_sql_capabilities() {
    assert!(
        DRIVERS
            .iter()
            .any(|driver| driver.kind == DatabaseKind::MongoDb && driver.name == "mongodb")
    );
    assert!(
        drivers_in(DatabaseCategory::NonRelational)
            .any(|driver| driver.kind == DatabaseKind::MongoDb)
    );
    assert_eq!(
        interaction_model(DatabaseKind::MongoDb),
        InteractionModel::Document
    );
    assert_eq!(
        SqlDialect::try_for_database_kind(DatabaseKind::MongoDb),
        None
    );
    let capabilities = DatabaseCapabilities::for_kind(DatabaseKind::MongoDb);
    assert!(capabilities.catalog);
    assert!(!capabilities.relation_ddl);
    assert!(!capabilities.relation_edit);
    assert!(!capabilities.manual_transactions);
    assert_eq!(
        ConnectionUrlFormat::default_for(DatabaseKind::MongoDb),
        ConnectionUrlFormat::MongoDb
    );
    let serialized = serde_json::to_string(&DatabaseKind::MongoDb).unwrap();
    assert_eq!(serialized, "\"mongodb\"");
}

#[test]
fn valkey_is_registered_as_a_key_value_driver_without_sql_catalog() {
    assert!(DRIVERS.iter().any(|driver| {
        driver.kind == DatabaseKind::Valkey
            && driver.name == "valkey"
            && driver.display_name == "Valkey"
    }));
    assert!(
        drivers_in(DatabaseCategory::NonRelational)
            .any(|driver| driver.kind == DatabaseKind::Valkey)
    );
    assert_eq!(
        interaction_model(DatabaseKind::Valkey),
        InteractionModel::KeyValue
    );
    assert_eq!(
        SqlDialect::try_for_database_kind(DatabaseKind::Valkey),
        None
    );
    let serialized = serde_json::to_string(&DatabaseKind::Valkey).unwrap();
    assert_eq!(serialized, "\"valkey\"");
}

#[test]
fn mongodb_uri_import_preserves_seeds_options_and_transient_credentials() {
    let imported = import_connection_url(
        "mongodb://alice:p%40ss@db-a:27017,db-b:27018/app?authSource=admin&replicaSet=rs0&readPreferenceTags=dc%3Awest&readPreferenceTags=use%3Areporting",
        Some("test"),
    ).unwrap();
    assert_eq!(imported.profile.kind, DatabaseKind::MongoDb);
    assert_eq!(imported.profile.database.as_deref(), Some("app"));
    assert_eq!(
        imported
            .profile
            .mongodb_options
            .as_ref()
            .unwrap()
            .seeds
            .len(),
        2
    );
    assert_eq!(
        imported
            .profile
            .mongodb_options
            .as_ref()
            .unwrap()
            .uri_options
            .len(),
        4
    );
    assert!(imported.transient_password.is_some());
    let formatted = format_connection_url(&imported.profile, ConnectionUrlFormat::MongoDb).unwrap();
    assert!(!formatted.contains("p%40ss"));
    assert!(formatted.contains("db-a:27017,db-b:27018"));
    let reparsed = import_connection_url(&formatted, None).unwrap();
    assert_eq!(
        reparsed.profile.mongodb_options,
        imported.profile.mongodb_options
    );
}

#[test]
fn mongodb_srv_requires_host_and_defaults_tls_without_inventing_a_port() {
    let imported = import_connection_url(
        "mongodb+srv://cluster.example.test/app?authSource=admin",
        None,
    )
    .unwrap();
    assert_eq!(imported.profile.kind, DatabaseKind::MongoDb);
    assert_eq!(imported.profile.port, None);
    let options = imported.profile.mongodb_options.unwrap();
    assert!(options.srv);
    assert!(
        matches!(options.seeds.as_slice(), [seed] if seed.host == "cluster.example.test" && seed.port.is_none())
    );
}

#[test]
fn mongodb_scheme_is_case_insensitive_and_query_plus_is_literal() {
    let imported = import_connection_url(
        "MONGODB://cluster.example.test/app?appName=release+candidate",
        None,
    )
    .unwrap();
    assert_eq!(
        imported.profile.mongodb_options.unwrap().uri_options,
        vec![("appName".to_owned(), "release+candidate".to_owned())]
    );
}

#[test]
fn mongodb_uri_rejects_secrets_in_options_and_duplicate_singletons() {
    assert!(
        import_connection_url(
            "mongodb://db/app?tlsCertificateKeyFilePassword=secret",
            None
        )
        .is_err()
    );
    assert!(import_connection_url("mongodb://db/app?replicaSet=a&replicaSet=b", None).is_err());
    assert!(import_connection_url("mongodb+srv://db:27017/app", None).is_err());
}

#[test]
fn mongodb_uri_import_survives_profile_draft_edit_and_save() {
    let imported = import_connection_url(
        "mongodb://db-a:27017,db-b:27018/app?replicaSet=rs0&readPreferenceTags=dc%3Awest",
        Some("replica"),
    )
    .unwrap();
    let draft = lazydb::model::profile_manager::ProfileDraft::edit(&imported.profile, false);
    let saved = draft.validate(&[]).unwrap().profile;
    assert_eq!(saved.mongodb_options, imported.profile.mongodb_options);
    let formatted = format_connection_url(&saved, saved.url_format).unwrap();
    assert!(formatted.contains("db-a:27017,db-b:27018"));
    assert!(formatted.contains("replicaSet=rs0"));
}

#[test]
fn mongodb_srv_profile_draft_saves_without_a_port() {
    let imported = import_connection_url(
        "mongodb+srv://cluster.example.test/app?authSource=admin",
        Some("srv"),
    )
    .unwrap();
    let saved = lazydb::model::profile_manager::ProfileDraft::edit(&imported.profile, false)
        .validate(&[])
        .unwrap()
        .profile;
    assert_eq!(saved.port, None);
    assert_eq!(saved.mongodb_options.as_ref().unwrap().seeds[0].port, None);
    assert!(
        format_connection_url(&saved, saved.url_format)
            .unwrap()
            .starts_with("mongodb+srv://cluster.example.test/")
    );
}

#[test]
fn mongodb_tls_disabled_and_required_are_preserved_in_uri_options() {
    let disabled =
        import_connection_url("mongodb://db.example.test/app?tls=false", Some("plain")).unwrap();
    assert_eq!(disabled.profile.ssl_mode, lazydb::profile::SslMode::Disable);

    let required =
        import_connection_url("mongodb://db.example.test/app?tls=true", Some("tls")).unwrap();
    assert_eq!(required.profile.ssl_mode, lazydb::profile::SslMode::Require);
}

#[test]
fn new_mongodb_profile_draft_has_a_formatable_single_seed() {
    let mut draft = lazydb::model::profile_manager::ProfileDraft::new(DatabaseKind::MongoDb);
    draft.name.set("local");
    draft.database.set("app");
    assert_eq!(draft.mongodb_options.as_ref().unwrap().seeds.len(), 1);
    let validated = draft.validate(&[]).unwrap();
    assert_eq!(validated.profile.port, Some(27017));
    assert!(
        format_connection_url(&validated.profile, validated.profile.url_format)
            .unwrap()
            .starts_with("mongodb://localhost:27017")
    );
}

#[test]
fn mongodb_profile_options_persist_without_persisting_uri_passwords() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("connections.toml");
    let store = ProfileStore::new(path.clone());
    let imported = import_connection_url(
        "mongodb://alice:private@db-a:27017,db-b:27018/app?replicaSet=rs0",
        Some("replica"),
    )
    .unwrap();
    let mut saved = imported.profile;
    saved.name = "replica".into();
    store.save(&[saved.clone()]).unwrap();
    let text = std::fs::read_to_string(path).unwrap();
    assert!(!text.contains("private"));
    assert_eq!(store.load().unwrap(), vec![saved]);
}
