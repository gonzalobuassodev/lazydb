use lazydb::{
    mongodb_command::{
        MongoAuthorizationError, MongoWritePolicy, authorize_command, parse_command,
    },
    profile::{Environment, import_connection_url},
};

fn profile(read_only: bool, environment: Environment) -> lazydb::profile::ConnectionProfile {
    let mut profile = import_connection_url("mongodb://localhost/app", Some("mongo"))
        .unwrap()
        .profile;
    profile.read_only = read_only;
    profile.environment = environment;
    profile
}

#[test]
fn read_only_profile_and_default_policy_reject_write_commands() {
    let command = parse_command(r#"{"insert":"items","documents":[{"x":1}]}"#, "app").unwrap();
    assert_eq!(
        authorize_command(
            &profile(true, Environment::Development),
            &command,
            MongoWritePolicy::All
        ),
        Err(MongoAuthorizationError::ProfileReadOnly)
    );
    assert_eq!(
        authorize_command(
            &profile(false, Environment::Development),
            &command,
            MongoWritePolicy::Deny
        ),
        Err(MongoAuthorizationError::PolicyDenied)
    );
}

#[test]
fn scope_and_environment_policy_are_enforced() {
    let read = parse_command(r#"{"find":"items"}"#, "other").unwrap();
    assert_eq!(
        authorize_command(
            &profile(false, Environment::Development),
            &read,
            MongoWritePolicy::All
        ),
        Err(MongoAuthorizationError::DatabaseScope)
    );
    let write = parse_command(r#"{"drop":"items"}"#, "app").unwrap();
    assert_eq!(
        authorize_command(
            &profile(false, Environment::Production),
            &write,
            MongoWritePolicy::NonProduction
        ),
        Err(MongoAuthorizationError::ProductionDenied)
    );
    assert_eq!(
        authorize_command(
            &profile(false, Environment::Production),
            &write,
            MongoWritePolicy::All
        ),
        Ok(())
    );
}

#[test]
fn selected_database_read_commands_are_allowed() {
    let command = parse_command(r#"{"find":"items","filter":{}}"#, "app").unwrap();
    assert_eq!(
        authorize_command(
            &profile(false, Environment::Production),
            &command,
            MongoWritePolicy::Deny
        ),
        Ok(())
    );
}

#[test]
fn cross_database_rename_and_aggregate_output_are_rejected() {
    let profile = profile(false, Environment::Development);
    let rename = parse_command(
        r#"{"renameCollection":"app.items","to":"other.archive"}"#,
        "admin",
    )
    .unwrap();
    // The command executes in admin, so it is already outside this profile's selected db.
    assert_eq!(
        authorize_command(&profile, &rename, MongoWritePolicy::All),
        Err(MongoAuthorizationError::DatabaseScope)
    );
    let output = parse_command(
        r#"{"aggregate":"items","pipeline":[{"$out":{"db":"other","coll":"archive"}}]}"#,
        "app",
    )
    .unwrap();
    assert_eq!(
        authorize_command(&profile, &output, MongoWritePolicy::All),
        Err(MongoAuthorizationError::NamespaceScope)
    );
}

#[test]
fn cross_database_lookup_is_rejected_by_namespace_policy() {
    let profile = profile(false, Environment::Development);
    let command = parse_command(r#"{"aggregate":"items","pipeline":[{"$lookup":{"from":{"db":"other","coll":"customers"},"as":"c","pipeline":[]}}]}"#, "app").unwrap();
    assert_eq!(
        authorize_command(&profile, &command, MongoWritePolicy::Deny),
        Err(MongoAuthorizationError::NamespaceScope)
    );
}

#[test]
fn rename_collection_requires_explicit_database_for_both_endpoints() {
    let profile = profile(false, Environment::Development);
    let command =
        parse_command(r#"{"renameCollection":"items","to":"app.archive"}"#, "app").unwrap();
    assert_eq!(
        authorize_command(&profile, &command, MongoWritePolicy::All),
        Err(MongoAuthorizationError::NamespaceScope)
    );
}
