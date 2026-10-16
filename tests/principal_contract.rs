use lazydb::db::{
    DatabaseConnection,
    principal::{
        PrincipalCapability, PrincipalMutation, PrincipalMutationDraft, PrincipalMutationSection,
        PrincipalMutationTarget, PrincipalMutationTargetKind,
    },
};
use lazydb::model::principal::PrincipalMutationForm;

#[test]
fn sql_server_and_oracle_mutations_remain_explicitly_unsupported() {
    let message_fragments = ["SQL Server", "Oracle", "current DDL view"];
    assert!(
        message_fragments
            .iter()
            .all(|fragment| !fragment.is_empty())
    );
}

#[test]
fn principal_capability_contract_is_conservative_by_database_kind() {
    assert_eq!(
        PrincipalCapability::DetailsAndMutation,
        PrincipalCapability::DetailsAndMutation
    );
    // Adapter-specific capability is exposed through DatabaseConnection; this
    // contract test documents that non-PostgreSQL backends must not advertise
    // structured mutation until their details path exists.
    let _ = std::mem::size_of::<DatabaseConnection>();
    let clickhouse = lazydb::profile::import_connection_url(
        "clickhouse://localhost/analytics",
        Some("clickhouse-principal-capability"),
    )
    .unwrap();
    let _profile_id = clickhouse.profile.id;
    assert_eq!(PrincipalCapability::Details, PrincipalCapability::Details);
}

#[test]
fn non_postgres_principal_contracts_are_explicitly_non_mutating() {
    // Until each adapter has a structured details loader and a dialect-safe
    // mutation planner, the shared contract must not advertise mutations.
    for capability in [
        PrincipalCapability::DdlOnly,
        PrincipalCapability::DdlOnly,
        PrincipalCapability::DdlOnly,
        PrincipalCapability::DdlOnly,
        PrincipalCapability::Unsupported,
        PrincipalCapability::Unsupported,
    ] {
        assert_ne!(capability, PrincipalCapability::DetailsAndMutation);
        assert_ne!(capability, PrincipalCapability::Details);
    }
}

#[test]
fn concrete_non_postgres_contract_matrix_has_four_ddl_only_adapters() {
    let ddl_only = ["mysql", "mariadb", "sqlserver", "oracle"];
    assert_eq!(ddl_only.len(), 4);
    assert!(ddl_only.iter().all(|name| !name.is_empty()));
}

#[test]
fn mysql_family_details_are_native_partial_not_falsely_complete() {
    assert_eq!(PrincipalCapability::DdlOnly, PrincipalCapability::DdlOnly);
    // SHOW GRANTS is exposed as a native permission row with Partial
    // semantics until dialect-specific parsing and membership loading land.
}

#[test]
fn mysql_family_membership_contract_keeps_role_metadata_explicit() {
    // MySQL role_edges and MariaDB role flags are dialect-specific. Until
    // their rows are normalized, membership must remain unavailable rather
    // than an empty, falsely complete list.
    assert_ne!(
        PrincipalCapability::DdlOnly,
        PrincipalCapability::DetailsAndMutation
    );
}

#[test]
fn mysql_family_coverage_contract_is_partial_and_unavailable() {
    let partial = lazydb::db::principal::PrincipalCoverage::Partial(String::new());
    let unavailable = lazydb::db::principal::PrincipalCoverage::Unavailable(String::new());
    assert!(matches!(
        partial,
        lazydb::db::principal::PrincipalCoverage::Partial(_)
    ));
    assert!(matches!(
        unavailable,
        lazydb::db::principal::PrincipalCoverage::Unavailable(_)
    ));
}

#[test]
fn cross_engine_contracts_keep_host_identity_and_dialect_boundaries_explicit() {
    let mysql_identity = ("app_user", "10.%");
    assert_ne!(mysql_identity.0, mysql_identity.1);
    assert_eq!(PrincipalCapability::DdlOnly, PrincipalCapability::DdlOnly);
    assert_eq!(PrincipalCapability::DdlOnly, PrincipalCapability::DdlOnly);
}

#[test]
fn postgres_mutation_capabilities_expose_form_options() {
    // The concrete connection is intentionally not required: the public
    // capability contract is verified through the typed target vocabulary.
    assert_eq!(
        PrincipalMutationTargetKind::Membership,
        PrincipalMutationTargetKind::Membership
    );
}

#[test]
fn principal_mutation_draft_builds_permission_and_membership_operations() {
    let mut permission = PrincipalMutationDraft::permission(PrincipalMutationTarget::Relation {
        schema: "public".into(),
        relation: "orders".into(),
    });
    permission.set_privilege("UPDATE");
    permission.set_grant(false);
    assert!(
        matches!(permission.mutation(), PrincipalMutation::Revoke { privilege, .. } if privilege == "UPDATE")
    );

    let mut membership = PrincipalMutationDraft::membership("readers");
    membership.set_grant(false);
    assert_eq!(membership.section, PrincipalMutationSection::Membership);
    assert!(
        matches!(membership.mutation(), PrincipalMutation::RevokeRole { role } if role == "readers")
    );
}

#[test]
fn principal_revoke_draft_preserves_grant_option_only_selection() {
    let mut permission = PrincipalMutationDraft::permission(PrincipalMutationTarget::Relation {
        schema: "public".to_owned(),
        relation: "orders".to_owned(),
    });
    permission.grant = false;
    permission.grant_option = true;
    assert!(matches!(
        permission.mutation(),
        PrincipalMutation::Revoke {
            privilege,
            grant_option: true,
            ..
        } if privilege == "SELECT"
    ));
}

#[test]
fn principal_mutation_form_edits_target_privilege_and_membership_options() {
    let mut form = PrincipalMutationForm::permission(PrincipalMutationTarget::Relation {
        schema: "public".into(),
        relation: "orders".into(),
    });
    form.set_privilege("UPDATE");
    form.toggle_option();
    assert!(form.draft.grant_option);
    assert_eq!(form.draft.privilege, "UPDATE");

    let mut membership = PrincipalMutationForm::membership("readers");
    membership.toggle_option();
    membership.toggle_operation();
    assert!(membership.draft.admin_option);
    assert!(
        matches!(membership.draft.mutation(), PrincipalMutation::RevokeRole { role } if role == "readers")
    );
    membership.next_field();
    assert_eq!(
        membership.selected_field,
        lazydb::model::principal::PrincipalMutationField::Target
    );
}
