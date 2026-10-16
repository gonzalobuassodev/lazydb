use lazydb::persistence::mongodb_workspace::{
    MongoWorkspaceStore, PersistedMongoConsole, PersistedMongoWorkspace,
};
use tempfile::TempDir;
use uuid::Uuid;

#[test]
fn mongo_workspace_restores_static_command_and_target_without_execution_state() {
    let temp = TempDir::new().unwrap();
    let store = MongoWorkspaceStore::new(temp.path().join("mongodb"));
    let id = Uuid::new_v4();
    let profile = Uuid::new_v4();
    let snapshot = PersistedMongoWorkspace {
        consoles: vec![PersistedMongoConsole {
            id,
            profile_id: profile,
            database: "app".into(),
            command_file: format!("{id}.mongodb.json").into(),
            open: true,
        }],
        commands: vec![(id, r#"{"find":"items","filter":{}}"#.into())],
    };
    store.save(&snapshot).unwrap();
    let restored = store.load().unwrap().unwrap();
    assert_eq!(restored, snapshot);
}

#[test]
fn mongo_workspace_rejects_path_traversal_and_removes_deleted_console_file() {
    let temp = TempDir::new().unwrap();
    let store = MongoWorkspaceStore::new(temp.path().join("mongodb"));
    let id = Uuid::new_v4();
    let entry = |command_file: String| PersistedMongoConsole {
        id,
        profile_id: Uuid::new_v4(),
        database: "app".into(),
        command_file: command_file.into(),
        open: true,
    };
    let bad = PersistedMongoWorkspace {
        consoles: vec![entry("../escape.json".into())],
        commands: vec![],
    };
    assert!(store.save(&bad).is_err());
    let good = PersistedMongoWorkspace {
        consoles: vec![entry(format!("{id}.mongodb.json"))],
        commands: vec![(id, "{}".into())],
    };
    store.save(&good).unwrap();
    store
        .save(&PersistedMongoWorkspace {
            consoles: vec![],
            commands: vec![],
        })
        .unwrap();
    assert!(
        !temp
            .path()
            .join("mongodb")
            .join(format!("{id}.mongodb.json"))
            .exists()
    );
}

#[test]
fn closed_mongo_console_keeps_command_file_but_does_not_restore_as_open() {
    let temp = TempDir::new().unwrap();
    let store = MongoWorkspaceStore::new(temp.path().join("mongodb"));
    let id = Uuid::new_v4();
    let snapshot = PersistedMongoWorkspace {
        consoles: vec![PersistedMongoConsole {
            id,
            profile_id: Uuid::new_v4(),
            database: "app".into(),
            command_file: format!("{id}.mongodb.json").into(),
            open: false,
        }],
        commands: vec![(id, r#"{"find":"items"}"#.into())],
    };
    store.save(&snapshot).unwrap();
    let file = temp
        .path()
        .join("mongodb")
        .join(format!("{id}.mongodb.json"));
    assert!(file.exists());
    let restored = store.load().unwrap().unwrap();
    assert_eq!(restored.consoles, snapshot.consoles);
    assert_eq!(restored.commands, snapshot.commands);
}
