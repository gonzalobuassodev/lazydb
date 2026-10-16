use lazydb::db::mongodb::principal::{MongoPrincipalMutation, validate_management_command};
use secrecy::SecretString;

#[test]
fn principal_management_is_read_only_and_rejects_database_override() {
    assert!(validate_management_command(&mongodb::bson::doc! {"usersInfo": 1}, "app").is_ok());
    assert!(
        validate_management_command(&mongodb::bson::doc! {"usersInfo": 1, "$db": "admin"}, "app")
            .is_err()
    );
    assert!(validate_management_command(&mongodb::bson::doc! {"dropUser": "test"}, "app").is_err());
}

#[test]
fn principal_mutation_keeps_password_ephemeral_and_commands_scoped() {
    let mutation = MongoPrincipalMutation::new(
        "app",
        "createUser",
        "reader",
        Some(SecretString::from("ephemeral-secret")),
        vec![mongodb::bson::doc! { "role": "read", "db": "app" }],
        Vec::new(),
    )
    .unwrap();
    assert!(mutation.has_password());
    assert_eq!(mutation.command_name(), Some("createUser"));
    assert!(!mutation.command_contains_field("pwd"));
    assert!(!format!("{mutation:?}").contains("ephemeral-secret"));
}

#[tokio::test]
async fn principal_mutations_apply_only_to_isolated_server_and_cleanup() {
    use mongodb::{Client, bson::doc};
    let Some(url) = support::mongodb_principals_url() else {
        return;
    };
    let client = Client::with_uri_str(&url).await.unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let user = format!("lazydb_user_{suffix}");
    let role = format!("lazydb_role_{suffix}");
    let cleanup_client = client.clone();
    let cleanup_user = user.clone();
    let cleanup_role = role.clone();
    let operation = async {
        let password = SecretString::from("ephemeral-integration-password");
        let grant = vec![doc! { "role": "read", "db": "app" }];
        let create_user = MongoPrincipalMutation::new(
            "admin",
            "createUser",
            &user,
            Some(password),
            grant.clone(),
            Vec::new(),
        )
        .unwrap();
        create_user
            .execute_for_database(&client, "admin", false)
            .await?;
        let add_role = MongoPrincipalMutation::new(
            "admin",
            "grantRolesToUser",
            &user,
            None,
            vec![doc! {"role": "readWrite", "db": "app"}],
            Vec::new(),
        )
        .unwrap();
        add_role
            .execute_for_database(&client, "admin", false)
            .await?;
        let create_role = MongoPrincipalMutation::new(
            "admin",
            "createRole",
            &role,
            None,
            Vec::new(),
            vec![doc! {"resource": {"db":"app", "collection":""}, "actions":["find"]}],
        )
        .unwrap();
        create_role
            .execute_for_database(&client, "admin", false)
            .await?;
        let drop_role =
            MongoPrincipalMutation::new("admin", "dropRole", &role, None, Vec::new(), Vec::new())
                .unwrap();
        drop_role
            .execute_for_database(&client, "admin", false)
            .await?;
        let drop_user =
            MongoPrincipalMutation::new("admin", "dropUser", &user, None, Vec::new(), Vec::new())
                .unwrap();
        drop_user
            .execute_for_database(&client, "admin", false)
            .await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    let result = operation.await;
    let _ = cleanup_client
        .database("admin")
        .run_command(doc! {"dropUser": cleanup_user})
        .await;
    let _ = cleanup_client
        .database("admin")
        .run_command(doc! {"dropRole": cleanup_role})
        .await;
    result.unwrap();
}

mod support;
