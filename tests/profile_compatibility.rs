use lazydb::{
    persistence::profiles::ProfileStore, profile::import_connection_url,
    profile_compatibility::ProfileUnavailableReason,
};
use tempfile::TempDir;

#[test]
fn load_report_keeps_supported_profiles_and_classifies_unknown_kind() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("connections.toml");
    let profile = import_connection_url("sqlite::memory:", Some("supported"))
        .unwrap()
        .profile;
    ProfileStore::new(path.clone()).save(vec![profile]).unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        format!(
            "{contents}\n[[profiles]]\nid = \"00000000-0000-0000-0000-000000000099\"\nname = \"future\"\nkind = \"future-db\"\nfuture_option = \"keep me\"\n"
        ),
    )
    .unwrap();

    let report = ProfileStore::new(path).load_report().unwrap();

    assert_eq!(report.collection.profiles.len(), 1);
    assert_eq!(report.unavailable.len(), 1);
    assert_eq!(report.unavailable[0].name, "future");
    assert_eq!(
        report.unavailable[0].reason,
        ProfileUnavailableReason::UnsupportedKind
    );
}

#[test]
fn load_report_classifies_redis_with_future_fields_as_unsupported_configuration() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("connections.toml");
    let profile = import_connection_url("redis://localhost:6379/0", Some("redis"))
        .unwrap()
        .profile;
    ProfileStore::new(path.clone()).save(vec![profile]).unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        contents.replace(
            "kind = \"redis\"",
            "kind = \"redis\"\nfuture_option = \"keep me\"",
        ),
    )
    .unwrap();

    let report = ProfileStore::new(path).load_report().unwrap();

    assert!(report.collection.profiles.is_empty());
    assert_eq!(report.unavailable.len(), 1);
    assert_eq!(report.unavailable[0].kind.as_deref(), Some("redis"));
    assert_eq!(
        report.unavailable[0].reason,
        ProfileUnavailableReason::UnsupportedConfiguration
    );
}

#[test]
fn valkey_profile_persists_and_reloads_without_losing_its_kind() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("connections.toml");
    let profile = import_connection_url("valkey://localhost:6379/2", Some("valkey"))
        .unwrap()
        .profile;
    ProfileStore::new(path.clone())
        .save(vec![profile.clone()])
        .unwrap();
    let loaded = ProfileStore::new(path).load().unwrap();
    assert_eq!(loaded.profiles, vec![profile]);
    assert_eq!(
        loaded.profiles[0].kind,
        lazydb::profile::DatabaseKind::Valkey
    );
}

#[test]
fn saving_supported_profiles_does_not_drop_unknown_profile_tables() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("connections.toml");
    let profile = import_connection_url("sqlite::memory:", Some("supported"))
        .unwrap()
        .profile;
    ProfileStore::new(path.clone())
        .save(vec![profile.clone()])
        .unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        format!(
            "{contents}\n[[profiles]]\nid = \"00000000-0000-0000-0000-000000000099\"\nname = \"future\"\nkind = \"future-db\"\nfuture_option = \"keep me\"\n"
        ),
    )
    .unwrap();

    ProfileStore::new(path.clone()).save(vec![profile]).unwrap();
    let saved = std::fs::read_to_string(path).unwrap();

    assert!(saved.contains("name = \"future\""));
    assert!(saved.contains("kind = \"future-db\""));
    assert!(saved.contains("future_option = \"keep me\""));
}

#[test]
fn saving_supported_profiles_preserves_known_kind_with_future_fields() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("connections.toml");
    let first = import_connection_url("sqlite::memory:", Some("first"))
        .unwrap()
        .profile;
    let middle = import_connection_url("sqlite::memory:", Some("middle"))
        .unwrap()
        .profile;
    let last = import_connection_url("sqlite::memory:", Some("last"))
        .unwrap()
        .profile;
    let mut profiles = vec![first, middle.clone(), last];
    let store = ProfileStore::new(path.clone());
    store.save(profiles.clone()).unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    let middle_id = middle.id.to_string();
    let middle_start = contents.find(&middle_id).unwrap();
    let kind_end = contents[middle_start..]
        .find("kind = \"sqlite\"\n")
        .map(|index| middle_start + index + "kind = \"sqlite\"\n".len())
        .unwrap();
    std::fs::write(
        &path,
        format!(
            "{}future_option = \"keep me\"\n[profiles.future_nested]\nmarker = \"nested-value\"\n[[profiles.future_nested.items]]\nmarker = \"array-value\"\n\n{}",
            &contents[..kind_end],
            &contents[kind_end..]
        ),
    )
    .unwrap();

    profiles.reverse();
    store.save(profiles).unwrap();
    let saved: toml::Value = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let middle_after = saved["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|profile| profile["id"].as_str() == Some(&middle_id))
        .unwrap();
    assert_eq!(middle_after["future_option"].as_str(), Some("keep me"));
    assert_eq!(
        middle_after["future_nested"]["marker"].as_str(),
        Some("nested-value")
    );
    assert_eq!(
        middle_after["future_nested"]["items"][0]["marker"].as_str(),
        Some("array-value")
    );
    for profile in saved["profiles"].as_array().unwrap() {
        if profile["id"].as_str() != Some(&middle_id) {
            assert!(profile.get("future_nested").is_none());
        }
    }
}

#[test]
fn deleting_middle_redis_does_not_restore_it_or_corrupt_profiles() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("connections.toml");
    let store = ProfileStore::new(path.clone());
    let first = import_connection_url("sqlite::memory:", Some("first"))
        .unwrap()
        .profile;
    let middle = import_connection_url("redis://localhost:6379/0", Some("middle"))
        .unwrap()
        .profile;
    let last = import_connection_url("sqlite::memory:", Some("last"))
        .unwrap()
        .profile;
    store
        .save(vec![first.clone(), middle, last.clone()])
        .unwrap();

    let remaining = vec![first, last];
    store.save(remaining.clone()).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    let parsed: toml::Value = toml::from_str(&saved).unwrap();
    assert_eq!(parsed["profiles"].as_array().unwrap().len(), 2);
    assert_eq!(store.load().unwrap().profiles, remaining);

    store.save(remaining.clone()).unwrap();
    assert_eq!(store.load().unwrap().profiles, remaining);
}

#[test]
fn preserved_unknown_profile_tables_keep_their_own_nested_values() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("connections.toml");
    let store = ProfileStore::new(path.clone());
    let first = import_connection_url("sqlite::memory:", Some("first"))
        .unwrap()
        .profile;
    let last = import_connection_url("sqlite::memory:", Some("last"))
        .unwrap()
        .profile;
    store.save(vec![first.clone(), last.clone()]).unwrap();

    let contents = std::fs::read_to_string(&path).unwrap();
    let future_id = "00000000-0000-0000-0000-000000000099";
    let second_profile = contents
        .match_indices("[[profiles]]")
        .nth(1)
        .map(|(index, _)| index)
        .unwrap();
    let future_profile = format!(
        "[[profiles]]\nid = \"{future_id}\"\nname = \"future\"\nkind = \"future-db\"\nfuture_option = \"future-root-value\"\n[[profiles.future_nested.items]]\nmarker = \"nested-sentinel\"\n\n"
    );
    std::fs::write(
        &path,
        format!(
            "{}{}{}",
            &contents[..second_profile],
            future_profile,
            &contents[second_profile..]
        ),
    )
    .unwrap();

    let original: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let future_before = original["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|profile| profile["id"].as_str() == Some(future_id))
        .unwrap()
        .clone();

    let mut remaining = vec![first, last];
    store.save(remaining.clone()).unwrap();
    let saved: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let profiles = saved["profiles"].as_array().unwrap();
    let future_after = profiles
        .iter()
        .find(|profile| profile["id"].as_str() == Some(future_id))
        .unwrap();
    assert_eq!(
        future_after["future_option"].as_str(),
        Some("future-root-value")
    );
    assert_eq!(
        future_after["future_nested"]["items"][0]["marker"].as_str(),
        Some("nested-sentinel")
    );
    for supported in &remaining {
        let saved_profile = profiles
            .iter()
            .find(|profile| profile["id"].as_str() == Some(&supported.id.to_string()))
            .unwrap();
        assert_eq!(
            saved_profile["name"].as_str(),
            Some(supported.name.as_str())
        );
    }

    remaining.reverse();
    store.save(remaining).unwrap();
    let repeated: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let future_after_repeat = repeated["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|profile| profile["id"].as_str() == Some(future_id))
        .unwrap();
    assert_eq!(
        future_after_repeat["future_option"],
        future_before["future_option"]
    );
    assert_eq!(
        future_after_repeat["future_nested"],
        future_before["future_nested"]
    );
    assert_eq!(store.load_report().unwrap().unavailable.len(), 1);
}

#[test]
fn malformed_existing_store_is_not_overwritten_by_save() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("connections.toml");
    let original = b"version = 6\n[[profiles]]\nid = \"broken\"\n[profiles.access]\nscope = \"global\"\n[profiles.access]\nscope = \"global\"\n";
    std::fs::write(&path, original).unwrap();
    let profile = import_connection_url("sqlite::memory:", Some("new"))
        .unwrap()
        .profile;

    assert!(ProfileStore::new(path.clone()).save(vec![profile]).is_err());
    assert_eq!(std::fs::read(path).unwrap(), original);
}
