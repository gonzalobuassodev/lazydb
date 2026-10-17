use std::sync::Mutex;

use lazydb::db::catalog::{
    CatalogEntry, CatalogId, CatalogKind, CatalogMetadata, CatalogTarget, ColumnMetadata,
    ObjectGroup, OptionalMetadata, QualifiedName,
};
use lazydb::lsp::catalog::{CatalogTargetLoader, discover_index};
use lazydb::profile::{CatalogScope, CatalogSelection, DatabaseScope};
use uuid::Uuid;

fn qualified(database: &str, schema: Option<&str>, object: &str) -> QualifiedName {
    QualifiedName {
        database: Some(database.into()),
        schema: schema.map(Into::into),
        object: object.into(),
    }
}

#[derive(Default)]
struct FakeLoader {
    responses: std::collections::HashMap<
        CatalogTarget,
        Result<Vec<CatalogEntry>, lazydb::db::DatabaseError>,
    >,
    calls: Mutex<Vec<(CatalogTarget, CatalogScope)>>,
}

impl FakeLoader {
    fn respond(&mut self, target: CatalogTarget, entries: Vec<CatalogEntry>) {
        self.responses.insert(target, Ok(entries));
    }

    fn respond_error(&mut self, target: CatalogTarget, message: &str) {
        self.responses.insert(
            target,
            Err(lazydb::db::DatabaseError::configuration(message)),
        );
    }

    fn respond_unsupported(&mut self, target: CatalogTarget, message: &str) {
        self.responses.insert(
            target,
            Err(lazydb::db::DatabaseError {
                category: lazydb::db::ErrorCategory::Unsupported,
                code: Some("catalog_target_unsupported".to_owned()),
                message: message.to_owned(),
                diagnostic: None,
            }),
        );
    }

    fn calls(&self) -> Vec<(CatalogTarget, CatalogScope)> {
        self.calls.lock().unwrap().clone()
    }

    fn respond_empty_targets(&mut self, schema: &CatalogId) {
        for group in [
            ObjectGroup::Tables,
            ObjectGroup::Views,
            ObjectGroup::MaterializedViews,
            ObjectGroup::Sequences,
            ObjectGroup::Types,
            ObjectGroup::Functions,
            ObjectGroup::Procedures,
        ] {
            self.respond(
                CatalogTarget::Objects {
                    schema: schema.clone(),
                    group,
                },
                Vec::new(),
            );
        }
    }
}

#[async_trait::async_trait]
impl CatalogTargetLoader for FakeLoader {
    async fn load(
        &self,
        target: CatalogTarget,
        scope: &CatalogScope,
    ) -> anyhow::Result<Vec<CatalogEntry>> {
        self.calls
            .lock()
            .unwrap()
            .push((target.clone(), scope.clone()));
        match self.responses.get(&target) {
            Some(Ok(entries)) => Ok(entries.clone()),
            Some(Err(error)) => Err(anyhow::Error::new(error.clone())),
            None => Err(anyhow::Error::new(
                lazydb::db::DatabaseError::configuration(format!(
                    "no configured response for {}",
                    target.description()
                )),
            )),
        }
    }
}

fn all_scope() -> CatalogScope {
    CatalogScope {
        databases: CatalogSelection::All,
    }
}

fn fixture_entries() -> (Uuid, Vec<CatalogEntry>) {
    let connection = Uuid::new_v4();
    let mut entries = Vec::new();
    for database_name in ["app", "other"] {
        let database = CatalogId::new(connection, CatalogKind::Database, [database_name]);
        entries.push(
            CatalogEntry::database(
                database.clone(),
                qualified(database_name, None, database_name),
                "database",
                OptionalMetadata::Supported(None),
                true,
            )
            .unwrap(),
        );
    }
    let app = CatalogId::new(connection, CatalogKind::Database, ["app"]);
    let public = CatalogId::new(connection, CatalogKind::Schema, ["app", "public"]);
    let audit = CatalogId::new(connection, CatalogKind::Schema, ["app", "audit"]);
    let empty = CatalogId::new(connection, CatalogKind::Schema, ["app", "empty"]);
    let other = CatalogId::new(connection, CatalogKind::Database, ["other"]);
    let tools = CatalogId::new(connection, CatalogKind::Schema, ["other", "tools"]);
    let users = CatalogId::new(connection, CatalogKind::Table, ["app", "public", "users"]);
    let audit_log = CatalogId::new(
        connection,
        CatalogKind::Table,
        ["app", "audit", "audit_log"],
    );
    let id = CatalogId::new(
        connection,
        CatalogKind::Column,
        ["app", "public", "users", "id"],
    );
    entries.push(
        CatalogEntry::schema(
            public.clone(),
            app.clone(),
            qualified("app", Some("public"), "public"),
            "schema",
            OptionalMetadata::Supported(None),
            true,
        )
        .unwrap(),
    );
    entries.push(
        CatalogEntry::schema(
            audit.clone(),
            app,
            qualified("app", Some("audit"), "audit"),
            "schema",
            OptionalMetadata::Supported(None),
            true,
        )
        .unwrap(),
    );
    entries.push(
        CatalogEntry::schema(
            empty.clone(),
            CatalogId::new(connection, CatalogKind::Database, ["app"]),
            qualified("app", Some("empty"), "empty"),
            "schema",
            OptionalMetadata::Supported(None),
            true,
        )
        .unwrap(),
    );
    entries.push(
        CatalogEntry::schema(
            tools.clone(),
            other,
            qualified("other", Some("tools"), "tools"),
            "schema",
            OptionalMetadata::Supported(None),
            true,
        )
        .unwrap(),
    );
    entries.push(
        CatalogEntry::relation(
            users.clone(),
            public.clone(),
            qualified("app", Some("public"), "users"),
            "table",
            OptionalMetadata::Supported(None),
            true,
        )
        .unwrap(),
    );
    entries.push(
        CatalogEntry::relation(
            audit_log,
            audit,
            qualified("app", Some("audit"), "audit_log"),
            "table",
            OptionalMetadata::Supported(None),
            true,
        )
        .unwrap(),
    );
    entries.push(
        CatalogEntry::relation_child(
            id,
            users,
            qualified("app", Some("public"), "id"),
            "column",
            OptionalMetadata::Unsupported,
            CatalogMetadata::Column(ColumnMetadata::new(1, "integer", false)),
        )
        .unwrap(),
    );
    (connection, entries)
}

#[tokio::test]
async fn discover_index_builds_full_hierarchy_with_real_ids() {
    let (connection, entries) = fixture_entries();
    let mut fake = FakeLoader::default();
    fake.respond(
        CatalogTarget::Databases,
        entries
            .iter()
            .filter(|entry| entry.kind == CatalogKind::Database)
            .cloned()
            .collect(),
    );
    for database_name in ["app", "other"] {
        let database = CatalogId::new(connection, CatalogKind::Database, [database_name]);
        fake.respond(
            CatalogTarget::Schemas { database },
            entries
                .iter()
                .filter(|entry| {
                    entry.kind == CatalogKind::Schema && entry.id.kind == CatalogKind::Schema
                })
                .filter(|entry| entry.id.native_path[0] == database_name)
                .cloned()
                .collect(),
        );
    }
    for schema in entries
        .iter()
        .filter(|entry| entry.kind == CatalogKind::Schema)
    {
        for group in [
            ObjectGroup::Tables,
            ObjectGroup::Views,
            ObjectGroup::MaterializedViews,
            ObjectGroup::Sequences,
            ObjectGroup::Types,
            ObjectGroup::Functions,
            ObjectGroup::Procedures,
        ] {
            fake.respond(
                CatalogTarget::Objects {
                    schema: schema.id.clone(),
                    group,
                },
                entries
                    .iter()
                    .filter(|entry| {
                        entry.kind.is_relation()
                            && entry.parent_id.as_ref() == Some(&schema.id)
                            && group.contains_kind(entry.id.kind)
                    })
                    .cloned()
                    .collect(),
            );
        }
    }
    for relation in entries.iter().filter(|entry| entry.id.kind.is_relation()) {
        fake.respond(
            CatalogTarget::RelationChildren {
                relation: relation.id.clone(),
            },
            entries
                .iter()
                .filter(|entry| entry.relation_id.as_ref() == Some(&relation.id))
                .cloned()
                .collect(),
        );
    }
    let scope = all_scope();
    let (index, status) = discover_index(&fake, &scope).await;

    assert!(
        status.complete,
        "all targets should load: {:?}",
        status.errors
    );
    assert!(status.errors.is_empty());
    let databases = index
        .entries()
        .iter()
        .filter(|entry| entry.kind == CatalogKind::Database)
        .collect::<Vec<_>>();
    assert_eq!(databases.len(), 2, "both databases must be indexed");
    let schemas = index
        .entries()
        .iter()
        .filter(|entry| entry.kind == CatalogKind::Schema)
        .map(|entry| entry.qualified_name.object.as_str())
        .collect::<Vec<_>>();
    for name in ["public", "audit", "empty", "tools"] {
        assert!(
            schemas.contains(&name),
            "schema {name} must be indexed even when empty"
        );
    }
    let public_schema = index
        .entries()
        .iter()
        .find(|entry| entry.qualified_name.object == "public")
        .expect("public schema");
    let app_database = index
        .entries()
        .iter()
        .find(|entry| entry.kind == CatalogKind::Database && entry.qualified_name.object == "app")
        .expect("app database")
        .id
        .clone();
    assert_eq!(
        public_schema.parent_id.as_ref(),
        Some(&app_database),
        "schema parent must be the real database id"
    );
    let users = index
        .entries()
        .iter()
        .find(|entry| entry.qualified_name.object == "users")
        .expect("users table");
    assert_eq!(
        users.parent_id.as_ref(),
        Some(&public_schema.id),
        "relation parent must be the real schema id"
    );
    assert!(
        index
            .entries()
            .iter()
            .any(|entry| entry.kind == CatalogKind::Column && entry.qualified_name.object == "id"),
        "columns must be indexed"
    );
}

#[tokio::test]
async fn failed_target_keeps_other_schemata_and_marks_incomplete() {
    let (connection, entries) = fixture_entries();
    let mut fake = FakeLoader::default();
    fake.respond(
        CatalogTarget::Databases,
        entries
            .iter()
            .filter(|entry| entry.kind == CatalogKind::Database)
            .cloned()
            .collect(),
    );
    fake.respond(
        CatalogTarget::Schemas {
            database: CatalogId::new(connection, CatalogKind::Database, ["app"]),
        },
        entries
            .iter()
            .filter(|entry| entry.kind == CatalogKind::Schema)
            .filter(|entry| entry.id.native_path[0] == "app")
            .cloned()
            .collect(),
    );
    fake.respond_error(
        CatalogTarget::Schemas {
            database: CatalogId::new(connection, CatalogKind::Database, ["other"]),
        },
        "boom",
    );
    for schema in entries
        .iter()
        .filter(|entry| entry.kind == CatalogKind::Schema && entry.id.native_path[0] == "app")
    {
        for group in [
            ObjectGroup::Tables,
            ObjectGroup::Views,
            ObjectGroup::MaterializedViews,
            ObjectGroup::Sequences,
            ObjectGroup::Types,
            ObjectGroup::Functions,
            ObjectGroup::Procedures,
        ] {
            fake.respond(
                CatalogTarget::Objects {
                    schema: schema.id.clone(),
                    group,
                },
                entries
                    .iter()
                    .filter(|entry| {
                        entry.kind.is_relation()
                            && entry.parent_id.as_ref() == Some(&schema.id)
                            && group.contains_kind(entry.id.kind)
                    })
                    .cloned()
                    .collect(),
            );
        }
    }
    fake.respond(
        CatalogTarget::RelationChildren {
            relation: CatalogId::new(connection, CatalogKind::Table, ["app", "public", "users"]),
        },
        vec![
            entries
                .iter()
                .find(|entry| entry.qualified_name.object == "id")
                .unwrap()
                .clone(),
        ],
    );
    fake.respond(
        CatalogTarget::RelationChildren {
            relation: CatalogId::new(
                connection,
                CatalogKind::Table,
                ["app", "audit", "audit_log"],
            ),
        },
        Vec::new(),
    );
    fake.respond(
        CatalogTarget::RelationChildren {
            relation: CatalogId::new(
                connection,
                CatalogKind::Table,
                ["other", "tools", "missing"],
            ),
        },
        Vec::new(),
    );

    let (index, status) = discover_index(&fake, &all_scope()).await;
    assert!(!status.complete);
    assert_eq!(status.errors.len(), 1);
    assert!(
        index
            .entries()
            .iter()
            .any(|entry| entry.qualified_name.object == "users"),
        "app schemata must survive other database failure"
    );
}

#[tokio::test]
async fn unsupported_group_is_skipped_without_failure() {
    let (connection, entries) = fixture_entries();
    let mut fake = FakeLoader::default();
    fake.respond(
        CatalogTarget::Databases,
        entries
            .iter()
            .filter(|entry| entry.kind == CatalogKind::Database)
            .cloned()
            .collect(),
    );
    fake.respond(
        CatalogTarget::Schemas {
            database: CatalogId::new(connection, CatalogKind::Database, ["app"]),
        },
        entries
            .iter()
            .filter(|entry| entry.kind == CatalogKind::Schema)
            .filter(|entry| entry.id.native_path[0] == "app")
            .cloned()
            .collect(),
    );
    fake.respond(
        CatalogTarget::Schemas {
            database: CatalogId::new(connection, CatalogKind::Database, ["other"]),
        },
        entries
            .iter()
            .filter(|entry| entry.kind == CatalogKind::Schema)
            .filter(|entry| entry.id.native_path[0] == "other")
            .cloned()
            .collect(),
    );
    for schema in entries
        .iter()
        .filter(|entry| entry.kind == CatalogKind::Schema)
    {
        fake.respond_empty_targets(&schema.id);
        fake.respond(
            CatalogTarget::Objects {
                schema: schema.id.clone(),
                group: ObjectGroup::Tables,
            },
            entries
                .iter()
                .filter(|entry| {
                    entry.kind.is_relation()
                        && entry.parent_id.as_ref() == Some(&schema.id)
                        && entry.id.kind == CatalogKind::Table
                })
                .cloned()
                .collect(),
        );
        fake.respond_unsupported(
            CatalogTarget::Objects {
                schema: schema.id.clone(),
                group: ObjectGroup::MaterializedViews,
            },
            "MaterializedViews catalog target is not implemented for MySql: catalog_target_unsupported",
        );
    }
    for relation in entries.iter().filter(|entry| entry.id.kind.is_relation()) {
        fake.respond(
            CatalogTarget::RelationChildren {
                relation: relation.id.clone(),
            },
            Vec::new(),
        );
    }

    let (index, status) = discover_index(&fake, &all_scope()).await;
    assert!(
        status.complete,
        "unsupported group must not mark discovery incomplete"
    );
    assert!(
        index
            .entries()
            .iter()
            .any(|entry| entry.qualified_name.object == "users"),
        "supported tables must still be indexed"
    );
}

#[tokio::test]
async fn scope_excludes_out_of_scope_relations() {
    let (connection, entries) = fixture_entries();
    let mut fake = FakeLoader::default();
    let app = CatalogId::new(connection, CatalogKind::Database, ["app"]);
    let public = CatalogId::new(connection, CatalogKind::Schema, ["app", "public"]);
    let secret = CatalogEntry::relation(
        CatalogId::new(
            connection,
            CatalogKind::Table,
            ["blocked", "public", "secret"],
        ),
        CatalogId::new(connection, CatalogKind::Schema, ["blocked", "public"]),
        qualified("blocked", Some("public"), "secret"),
        "table",
        OptionalMetadata::Supported(None),
        true,
    )
    .unwrap();
    let users_id = CatalogId::new(connection, CatalogKind::Table, ["app", "public", "users"]);
    let users = CatalogEntry::relation(
        users_id.clone(),
        public.clone(),
        qualified("app", Some("public"), "users"),
        "table",
        OptionalMetadata::Supported(None),
        true,
    )
    .unwrap();
    fake.respond(
        CatalogTarget::Databases,
        entries
            .iter()
            .filter(|entry| entry.kind == CatalogKind::Database)
            .filter(|entry| entry.qualified_name.object == "app")
            .cloned()
            .collect(),
    );
    fake.respond(
        CatalogTarget::Schemas {
            database: app.clone(),
        },
        entries
            .iter()
            .filter(|entry| entry.kind == CatalogKind::Schema && entry.id.native_path[0] == "app")
            .cloned()
            .collect(),
    );
    fake.respond(
        CatalogTarget::Objects {
            schema: public.clone(),
            group: ObjectGroup::Tables,
        },
        vec![secret, users],
    );
    for schema in entries
        .iter()
        .filter(|entry| entry.kind == CatalogKind::Schema && entry.id.native_path[0] == "app")
    {
        fake.respond_empty_targets(&schema.id);
    }
    fake.respond(
        CatalogTarget::RelationChildren {
            relation: CatalogId::new(
                connection,
                CatalogKind::Table,
                ["blocked", "public", "secret"],
            ),
        },
        Vec::new(),
    );
    fake.respond(
        CatalogTarget::RelationChildren { relation: users_id },
        Vec::new(),
    );
    let scope = CatalogScope {
        databases: CatalogSelection::Selected(vec![DatabaseScope {
            name: "app".into(),
            schemas: CatalogSelection::All,
        }]),
    };
    let (index, status) = discover_index(&fake, &scope).await;
    assert!(status.complete);
    assert!(
        !index
            .entries()
            .iter()
            .any(|entry| entry.qualified_name.object == "secret"),
        "out-of-scope relations must never be indexed"
    );
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|(target, _)| matches!(target, CatalogTarget::Databases))
            .count(),
        1,
        "databases must be requested exactly once"
    );
}
