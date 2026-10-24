use lazydb::{
    action::{Action, Command},
    app::App,
    db::redis::types::{RedisKeyId, RedisTarget},
    model::execution_target::ExecutionTarget,
    model::redis_key_tree::KeyTreeNodeId,
    model::workspace::ConnectionStatus,
    model::{
        redis_browser::{RedisPreviewState, RedisValuePageState},
        tab::WorkspaceTab,
    },
    persistence::workspace::{PersistedTab, WorkspaceStore},
    profile::{DatabaseKind, import_connection_url},
};
use tempfile::TempDir;
use uuid::Uuid;

fn redis_profile(name: &str) -> lazydb::profile::ConnectionProfile {
    import_connection_url("redis://localhost:6379", Some(name))
        .unwrap()
        .profile
}

#[test]
fn redis_preview_grid_resets_and_clamps_like_a_result_grid() {
    let mut tab = lazydb::model::redis_browser::RedisBrowserTab::new(
        Uuid::from_u128(100),
        lazydb::db::redis::types::RedisTarget {
            profile_id: Uuid::from_u128(101),
            database: 0,
        },
    );
    tab.preview_grid.selected_row = 9;
    tab.preview_grid.selected_column = 4;
    tab.preview_grid.row_offset = 8;
    tab.preview_grid.column_offset = 3;
    tab.preview_grid.viewport_rows = 3;
    tab.preview_grid.column_widths = vec![Some(8); 5];

    tab.clamp_preview_grid(4, 2);
    assert_eq!(tab.preview_grid.selected_row, 3);
    assert_eq!(tab.preview_grid.selected_column, 1);
    assert_eq!(tab.preview_grid.row_offset, 1);
    assert_eq!(tab.preview_grid.column_offset, 1);
    assert_eq!(tab.preview_grid.column_widths, vec![Some(8), Some(8)]);

    tab.reset_preview_grid();
    assert_eq!(tab.preview_grid, Default::default());
}

#[test]
fn redis_table_grid_snapshot_uses_raw_source_cells() {
    use lazydb::db::redis::read::{RedisKeyMetadata, RedisPagePosition, RedisType, TtlState};

    let profile_id = Uuid::from_u128(103);
    let mut app = App::new(Vec::new());
    let mut tab = lazydb::model::redis_browser::RedisBrowserTab::new(
        Uuid::from_u128(104),
        lazydb::db::redis::types::RedisTarget {
            profile_id,
            database: 0,
        },
    );
    tab.focus = lazydb::model::redis_browser::RedisBrowserFocus::Preview;
    tab.format
        .select(lazydb::value_preview::PreviewFormat::TABLE);
    tab.preview_grid.selected_row = 1;
    tab.preview_grid.selected_column = 1;
    tab.value_page = lazydb::model::redis_browser::RedisValuePageState::Ready(
        lazydb::db::redis::read::RedisValuePage {
            metadata: RedisKeyMetadata {
                key: lazydb::db::redis::types::RedisKeyId {
                    target: tab.target.clone(),
                    key: b"hash".to_vec(),
                },
                value_type: RedisType::Hash,
                ttl: TtlState::Persistent,
                memory_usage_bytes: None,
                value_size: Some(2),
            },
            position: RedisPagePosition::Complete,
            value: lazydb::db::redis::read::RedisPageValue::Hash(vec![
                (b"one".to_vec(), b"short".to_vec()),
                (b"two".to_vec(), vec![0xff, 0x00]),
            ]),
            truncated: false,
            complete: true,
            raw_bytes: 0,
            formatted_bytes: 0,
        },
    );
    app.tabs.push(WorkspaceTab::RedisBrowser(tab));
    app.active_tab = app.tabs.len() - 1;
    app.focus = lazydb::model::workspace::Focus::Results;

    let commands = app.update(lazydb::action::Action::CopyGridCell);
    assert!(
        matches!(
            commands.as_slice(),
            [lazydb::action::Command::WriteClipboard(payload)]
                if payload.text == r#"\xff\x00"#
        ),
        "commands: {commands:?}"
    );
}

#[test]
fn loading_the_next_redis_value_page_is_idempotent_and_stops_at_complete() {
    let profile_id = Uuid::from_u128(105);
    let mut app = App::new(Vec::new());
    app.connection.profile_id = Some(profile_id);
    app.connection.generation = 1;
    app.connection.target = Some(ExecutionTarget {
        profile_id,
        database: "0".into(),
        schema: None,
    });
    app.connection.status = ConnectionStatus::Connected;
    let mut tab = lazydb::model::redis_browser::RedisBrowserTab::new(
        Uuid::from_u128(106),
        RedisTarget {
            profile_id,
            database: 0,
        },
    );
    tab.value_page = RedisValuePageState::Ready(lazydb::db::redis::read::RedisValuePage {
        metadata: lazydb::db::redis::read::RedisKeyMetadata {
            key: RedisKeyId {
                target: tab.target.clone(),
                key: b"list".to_vec(),
            },
            value_type: lazydb::db::redis::read::RedisType::List,
            ttl: lazydb::db::redis::read::TtlState::Persistent,
            memory_usage_bytes: None,
            value_size: Some(2),
        },
        position: lazydb::db::redis::read::RedisPagePosition::ListOffset(2),
        value: lazydb::db::redis::read::RedisPageValue::List(vec![(0, b"a".to_vec())]),
        truncated: false,
        complete: false,
        raw_bytes: 1,
        formatted_bytes: 1,
    });
    app.tabs.push(WorkspaceTab::RedisBrowser(tab));
    app.active_tab = app.tabs.len() - 1;

    let first = app.update(Action::RedisPreviewLoadNext);
    assert!(matches!(
        first.as_slice(),
        [Command::LoadRedisValuePage { .. }]
    ));
    assert!(app.update(Action::RedisPreviewLoadNext).is_empty());

    let tab_id = app.tabs[app.active_tab].id();
    app.update(Action::RedisValuePageLoaded {
        tab_id,
        connection: app.connection.active_identity().unwrap(),
        preview_generation: 0,
        page: lazydb::db::redis::read::RedisValuePage {
            metadata: match &app.tabs[app.active_tab] {
                WorkspaceTab::RedisBrowser(tab) => match &tab.value_page {
                    RedisValuePageState::Ready(page) => page.metadata.clone(),
                    _ => unreachable!(),
                },
                _ => unreachable!(),
            },
            position: lazydb::db::redis::read::RedisPagePosition::Complete,
            value: lazydb::db::redis::read::RedisPageValue::List(vec![(2, b"b".to_vec())]),
            truncated: false,
            complete: true,
            raw_bytes: 1,
            formatted_bytes: 1,
        },
    });
    assert!(app.update(Action::RedisPreviewLoadNext).is_empty());
}

#[test]
fn appending_value_pages_merges_overlapping_collection_entries() {
    use lazydb::db::redis::read::{RedisPagePosition, RedisPageValue};

    let target = RedisTarget {
        profile_id: Uuid::from_u128(107),
        database: 0,
    };
    let mut tab = lazydb::model::redis_browser::RedisBrowserTab::new(Uuid::from_u128(108), target);
    let metadata = lazydb::db::redis::read::RedisKeyMetadata {
        key: RedisKeyId {
            target: tab.target.clone(),
            key: b"hash".to_vec(),
        },
        value_type: lazydb::db::redis::read::RedisType::Hash,
        ttl: lazydb::db::redis::read::TtlState::Persistent,
        memory_usage_bytes: None,
        value_size: Some(2),
    };
    tab.value_page = RedisValuePageState::Ready(lazydb::db::redis::read::RedisValuePage {
        metadata: metadata.clone(),
        position: RedisPagePosition::HashCursor(4),
        value: RedisPageValue::Hash(vec![(b"field".to_vec(), b"old".to_vec())]),
        truncated: false,
        complete: false,
        raw_bytes: 3,
        formatted_bytes: 3,
    });
    tab.append_value_page(lazydb::db::redis::read::RedisValuePage {
        metadata,
        position: RedisPagePosition::Complete,
        value: RedisPageValue::Hash(vec![
            (b"field".to_vec(), b"new".to_vec()),
            (b"other".to_vec(), b"value".to_vec()),
        ]),
        truncated: false,
        complete: true,
        raw_bytes: 8,
        formatted_bytes: 8,
    });

    let RedisValuePageState::Ready(page) = tab.value_page else {
        panic!("expected ready page");
    };
    assert_eq!(
        page.value,
        RedisPageValue::Hash(vec![
            (b"field".to_vec(), b"new".to_vec()),
            (b"other".to_vec(), b"value".to_vec()),
        ])
    );
    assert_eq!(page.position, RedisPagePosition::Complete);
}

#[test]
fn preview_controls_open_picker_and_apply_only_on_enter() {
    use lazydb::model::{
        redis_browser::{RedisBrowserFocus, RedisBrowserTab},
        workspace::Overlay,
    };
    let mut app = App::new(Vec::new());
    app.tabs
        .push(WorkspaceTab::RedisBrowser(RedisBrowserTab::new(
            Uuid::new_v4(),
            RedisTarget {
                profile_id: Uuid::nil(),
                database: 0,
            },
        )));
    app.active_tab = app.tabs.len() - 1;
    app.update(Action::RedisPreviewCycleFormat);
    assert!(matches!(
        app.overlay,
        Some(Overlay::RedisPreviewFormat { selected: 0 })
    ));
    app.update(Action::RedisPreviewFormatMove(1));
    let WorkspaceTab::RedisBrowser(tab) = &app.tabs[app.active_tab] else {
        panic!()
    };
    assert_eq!(
        tab.format.selected,
        lazydb::value_preview::PreviewFormat::RAW
    );
    assert!(tab.format.automatic);
    assert!(tab.preview_wrap);
    assert_eq!(tab.focus, RedisBrowserFocus::Preview);
    app.update(Action::RedisPreviewFormatAccept);
    assert!(app.overlay.is_none());
    app.update(Action::RedisPreviewToggleWrap);
    let WorkspaceTab::RedisBrowser(tab) = &app.tabs[app.active_tab] else {
        panic!()
    };
    assert_eq!(
        tab.format.selected,
        lazydb::value_preview::PreviewFormat::RAW
    );
    assert!(!tab.preview_wrap);
}

fn connect_redis(app: &mut App, profile_id: Uuid, database: &str) {
    let generation = match app.update(Action::RequestConnect(profile_id)).as_slice() {
        [Command::Connect { generation, .. }] => *generation,
        commands => panic!("unexpected commands: {commands:?}"),
    };
    app.update(Action::ConnectionSucceeded {
        profile_id,
        generation,
        server: lazydb::db::ServerInfo {
            kind: DatabaseKind::Redis,
            version: "7.2".into(),
            database: database.into(),
            current_user: None,
        },
        mutation_capabilities: Default::default(),
    });
}

#[test]
fn valkey_connection_uses_shared_database_discovery_without_sql_catalog() {
    let profile = import_connection_url("valkey://localhost:6379/0", Some("valkey"))
        .unwrap()
        .profile;
    let profile_id = profile.id;
    let mut app = App::new(vec![profile]);
    let generation = match app.update(Action::RequestConnect(profile_id)).as_slice() {
        [Command::Connect { generation, .. }] => *generation,
        commands => panic!("unexpected commands: {commands:?}"),
    };

    let commands = app.update(Action::ConnectionSucceeded {
        profile_id,
        generation,
        server: lazydb::db::ServerInfo {
            kind: DatabaseKind::Valkey,
            version: "8.1.0".into(),
            database: "0".into(),
            current_user: None,
        },
        mutation_capabilities: Default::default(),
    });

    assert!(commands.iter().any(|command| matches!(
        command,
        Command::DiscoverRedisDatabases { profile_id: id, generation: request_generation }
            if *id == profile_id && *request_generation == generation
    )));
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::LoadCatalogPage(_)))
    );

    let discovery = lazydb::db::redis::discovery::RedisDatabaseDiscovery {
        databases: vec![lazydb::db::redis::discovery::RedisDatabaseInfo {
            database: 2,
            keys: Some(1),
            expires: None,
        }],
        completeness: lazydb::db::redis::discovery::RedisDiscoveryCompleteness::Complete,
        warnings: Vec::new(),
    };
    app.update(Action::RedisDatabasesLoaded {
        profile_id,
        generation,
        discovery,
    });
    app.explorer
        .normalized
        .expanded
        .insert(lazydb::model::explorer::ExplorerNodeId::Profile(profile_id));
    let rows = app.explorer.visible();
    assert!(rows.iter().any(|row| matches!(
        &row.id,
        lazydb::model::explorer::ExplorerNodeId::RedisDatabase {
            profile_id: id,
            database: 2
        } if *id == profile_id
    )));
}

#[test]
fn opening_discovered_valkey_database_binds_browser_to_valkey_profile() {
    let profile = import_connection_url("valkey://localhost:6379/0", Some("valkey"))
        .unwrap()
        .profile;
    let profile_id = profile.id;
    let mut app = App::new(vec![profile]);
    let generation = match app.update(Action::RequestConnect(profile_id)).as_slice() {
        [Command::Connect { generation, .. }] => *generation,
        commands => panic!("unexpected commands: {commands:?}"),
    };
    app.update(Action::ConnectionSucceeded {
        profile_id,
        generation,
        server: lazydb::db::ServerInfo {
            kind: DatabaseKind::Valkey,
            version: "8.1.0".into(),
            database: "0".into(),
            current_user: None,
        },
        mutation_capabilities: Default::default(),
    });

    let commands = app.update(Action::OpenRedisDatabase {
        profile_id,
        database: 2,
    });
    let generation = commands
        .iter()
        .find_map(|command| match command {
            Command::Connect {
                generation, target, ..
            } if target.profile_id == profile_id && target.database == "2" => Some(*generation),
            _ => None,
        })
        .expect("opening a different logical database should connect to DB 2");
    let commands = app.update(Action::ConnectionSucceeded {
        profile_id,
        generation,
        server: lazydb::db::ServerInfo {
            kind: DatabaseKind::Valkey,
            version: "8.1.0".into(),
            database: "2".into(),
            current_user: None,
        },
        mutation_capabilities: Default::default(),
    });
    assert!(commands.iter().any(|command| matches!(
        command,
        Command::ScanRedisKeys(request)
            if request.identity.target.profile_id == profile_id
                && request.identity.target.database == 2
    )));
    let WorkspaceTab::RedisBrowser(tab) = app.tabs.last().expect("browser opened") else {
        panic!("expected Redis-compatible browser tab");
    };
    assert_eq!(tab.target.profile_id, profile_id);
    assert_eq!(tab.target.database, 2);
}

#[test]
fn opening_a_database_creates_one_empty_redis_browser_tab() {
    let profile_id = Uuid::from_u128(1);
    let mut app = App::new(Vec::new());
    app.connection.profile_id = Some(profile_id);
    app.connection.generation = 1;
    app.connection.target = Some(ExecutionTarget {
        profile_id,
        database: "2".into(),
        schema: None,
    });
    app.connection.status = ConnectionStatus::Connected;
    let commands = app.update(Action::OpenRedisDatabase {
        profile_id,
        database: 2,
    });
    assert_eq!(
        commands
            .iter()
            .filter(|command| matches!(command, lazydb::action::Command::ScanRedisKeys(_)))
            .count(),
        1
    );
    assert_eq!(app.tabs.len(), 2);
    let first_id = match app.tabs.last().unwrap() {
        WorkspaceTab::RedisBrowser(tab) => {
            assert_eq!(
                tab.target,
                RedisTarget {
                    profile_id,
                    database: 2
                }
            );
            assert_eq!(tab.preview, RedisPreviewState::Empty);
            tab.id
        }
        _ => panic!("expected Redis browser tab"),
    };
    app.update(Action::OpenRedisDatabase {
        profile_id,
        database: 2,
    });
    assert_eq!(app.tabs.len(), 2);
    assert!(app.tabs.iter().any(|tab| tab.id() == first_id));
}

#[test]
fn opening_redis_on_second_profile_produces_a_valid_snapshot() {
    let first = redis_profile("first");
    let second = redis_profile("second");
    let first_id = first.id;
    let second_id = second.id;
    let mut app = App::new(vec![first, second]);

    connect_redis(&mut app, first_id, "0");
    let commands = app.update(Action::OpenRedisDatabase {
        profile_id: second_id,
        database: 0,
    });
    let generation = commands
        .iter()
        .find_map(|command| match command {
            Command::Connect { generation, .. } => Some(*generation),
            _ => None,
        })
        .expect("opening the second profile should request its connection");
    app.update(Action::ConnectionSucceeded {
        profile_id: second_id,
        generation,
        server: lazydb::db::ServerInfo {
            kind: DatabaseKind::Redis,
            version: "7.2".into(),
            database: "0".into(),
            current_user: None,
        },
        mutation_capabilities: Default::default(),
    });
    app.update(Action::OpenRedisDatabase {
        profile_id: second_id,
        database: 0,
    });

    let redis_tabs = app
        .tabs
        .iter()
        .filter_map(|tab| match tab {
            WorkspaceTab::RedisBrowser(tab) => Some(tab),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(redis_tabs.len(), 1);
    assert_eq!(redis_tabs[0].target.profile_id, second_id);

    let snapshot = app.workspace_snapshot();
    let tab_count = snapshot
        .profiles
        .iter()
        .flat_map(|profile| profile.tabs.iter())
        .filter(|tab| matches!(tab, PersistedTab::RedisBrowser { .. }))
        .count();
    assert_eq!(tab_count, 1);

    let temp = TempDir::new().unwrap();
    let store = WorkspaceStore::new(temp.path().join("workspace.toml"), temp.path().join("sql"));
    store.save(&snapshot).unwrap();
}

#[test]
fn reopening_an_existing_not_loaded_tab_dispatches_its_first_scan() {
    let profile_id = Uuid::from_u128(101);
    let mut app = App::new(Vec::new());
    app.connection.profile_id = Some(profile_id);
    app.connection.generation = 3;
    app.connection.target = Some(ExecutionTarget {
        profile_id,
        database: "0".into(),
        schema: None,
    });
    app.connection.status = ConnectionStatus::Connected;
    let tab = lazydb::model::redis_browser::RedisBrowserTab::new(
        Uuid::from_u128(102),
        RedisTarget {
            profile_id,
            database: 0,
        },
    );
    app.tabs.push(WorkspaceTab::RedisBrowser(tab));
    app.active_tab = app.tabs.len() - 1;
    let commands = app.update(Action::OpenRedisDatabase {
        profile_id,
        database: 0,
    });
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, lazydb::action::Command::ScanRedisKeys(_)))
    );
}

#[test]
fn reopening_a_complete_empty_tab_does_not_repeat_scan() {
    let profile_id = Uuid::from_u128(121);
    let mut app = App::new(Vec::new());
    app.connection.profile_id = Some(profile_id);
    app.connection.generation = 1;
    app.connection.target = Some(ExecutionTarget {
        profile_id,
        database: "0".into(),
        schema: None,
    });
    app.connection.status = ConnectionStatus::Connected;
    let mut tab = lazydb::model::redis_browser::RedisBrowserTab::new(
        Uuid::from_u128(122),
        RedisTarget {
            profile_id,
            database: 0,
        },
    );
    tab.keyspace.position = lazydb::db::redis::types::ScanPosition::Complete;
    tab.keyspace.status = lazydb::model::keyspace::KeyspaceStatus::CompleteEmpty;
    app.tabs.push(WorkspaceTab::RedisBrowser(tab));
    app.active_tab = app.tabs.len() - 1;
    let commands = app.update(Action::OpenRedisDatabase {
        profile_id,
        database: 0,
    });
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, lazydb::action::Command::ScanRedisKeys(_)))
    );
}

#[test]
fn keyspace_status_distinguishes_initial_empty_partial_and_failure() {
    use lazydb::model::keyspace::{KeyspaceState, KeyspaceStatus};
    let target = RedisTarget {
        profile_id: Uuid::nil(),
        database: 0,
    };
    let mut state = KeyspaceState::new(Uuid::nil(), target, b"*".to_vec());
    assert_eq!(state.status, KeyspaceStatus::NotLoaded);
    state.status = KeyspaceStatus::Partial;
    assert_ne!(state.status, KeyspaceStatus::CompleteEmpty);
    state.status = KeyspaceStatus::Failed("NOPERM".into());
    assert!(matches!(state.status, KeyspaceStatus::Failed(_)));
}

#[test]
fn selecting_a_key_does_not_open_a_preview_but_explicit_open_does() {
    let mut app = App::new(Vec::new());
    app.connection.profile_id = Some(Uuid::from_u128(1));
    app.connection.generation = 1;
    app.connection.target = Some(ExecutionTarget {
        profile_id: Uuid::from_u128(1),
        database: "2".into(),
        schema: None,
    });
    app.connection.status = ConnectionStatus::Connected;
    app.update(Action::OpenRedisDatabase {
        profile_id: Uuid::from_u128(1),
        database: 2,
    });
    let tab_id = app.tabs.last().unwrap().id();
    if let Some(WorkspaceTab::RedisBrowser(tab)) = app.tabs.last_mut() {
        tab.tree.rebuild(&[RedisKeyId {
            target: tab.target.clone(),
            key: b"user:1".to_vec(),
        }]);
    }
    app.select_redis_key(tab_id, Some(KeyTreeNodeId::Key(b"user:1".to_vec())));
    assert!(app.update(Action::RedisPreviewTick).is_empty());
    app.select_redis_key(tab_id, Some(KeyTreeNodeId::Prefix(b"user:".to_vec())));
    assert!(
        matches!(app.tabs.last().unwrap(), WorkspaceTab::RedisBrowser(tab) if tab.preview == RedisPreviewState::Empty)
    );
    let commands = app.update(Action::OpenRedisKey {
        tab_id,
        node: KeyTreeNodeId::Key(b"user:1".to_vec()),
    });
    assert!(commands.is_empty());
    assert!(matches!(
        app.tabs.last().unwrap(),
        WorkspaceTab::RedisBrowser(tab) if matches!(tab.value_page, RedisValuePageState::Loading { .. })
    ));
    assert!(matches!(
        app.tabs.last().unwrap(),
        WorkspaceTab::RedisBrowser(tab) if tab.focus == lazydb::model::redis_browser::RedisBrowserFocus::Preview
    ));
}

#[test]
fn rapid_redis_key_open_dispatches_only_the_latest_preview() {
    let mut app = App::new(Vec::new());
    app.connection.profile_id = Some(Uuid::from_u128(1));
    app.connection.generation = 1;
    app.connection.target = Some(ExecutionTarget {
        profile_id: Uuid::from_u128(1),
        database: "2".into(),
        schema: None,
    });
    app.connection.status = ConnectionStatus::Connected;
    app.update(Action::OpenRedisDatabase {
        profile_id: Uuid::from_u128(1),
        database: 2,
    });
    let tab_id = app.tabs.last().unwrap().id();
    if let Some(WorkspaceTab::RedisBrowser(tab)) = app.tabs.last_mut() {
        tab.tree.rebuild(&[
            RedisKeyId {
                target: tab.target.clone(),
                key: b"a".to_vec(),
            },
            RedisKeyId {
                target: tab.target.clone(),
                key: b"b".to_vec(),
            },
        ]);
    }
    app.update(Action::OpenRedisKey {
        tab_id,
        node: KeyTreeNodeId::Key(b"a".to_vec()),
    });
    app.update(Action::OpenRedisKey {
        tab_id,
        node: KeyTreeNodeId::Key(b"b".to_vec()),
    });
    let commands = app.update(Action::RedisPreviewTick);
    assert_eq!(commands.len(), 1);
    assert!(matches!(
        &commands[0],
        lazydb::action::Command::LoadRedisValuePreview { key, .. } if key.key == b"b"
    ));
}

#[test]
fn opening_keys_advances_preview_generation_so_old_results_can_be_rejected() {
    let mut tab = lazydb::model::redis_browser::RedisBrowserTab::new(
        Uuid::from_u128(1),
        RedisTarget {
            profile_id: Uuid::from_u128(2),
            database: 2,
        },
    );
    tab.tree.rebuild(&[
        RedisKeyId {
            target: tab.target.clone(),
            key: b"a".to_vec(),
        },
        RedisKeyId {
            target: tab.target.clone(),
            key: b"b".to_vec(),
        },
    ]);
    tab.open_key(RedisKeyId {
        target: tab.target.clone(),
        key: b"a".to_vec(),
    });
    let first = tab.preview_generation;
    tab.open_key(RedisKeyId {
        target: tab.target.clone(),
        key: b"b".to_vec(),
    });
    assert!(first < tab.preview_generation);
}

#[test]
fn opening_a_key_starts_a_typed_value_page_lifecycle() {
    let mut tab = lazydb::model::redis_browser::RedisBrowserTab::new(
        Uuid::from_u128(1),
        RedisTarget {
            profile_id: Uuid::from_u128(2),
            database: 0,
        },
    );
    tab.tree.rebuild(&[RedisKeyId {
        target: tab.target.clone(),
        key: b"user:1".to_vec(),
    }]);
    tab.open_key(RedisKeyId {
        target: tab.target.clone(),
        key: b"user:1".to_vec(),
    });
    assert!(matches!(
        tab.value_page,
        RedisValuePageState::Loading { .. }
    ));
    tab.select(None);
    assert!(matches!(
        tab.value_page,
        RedisValuePageState::Loading { .. }
    ));
}

#[test]
fn redis_browser_focus_cycles_explorer_keys_preview() {
    let mut app = App::new(Vec::new());
    app.tabs.push(WorkspaceTab::RedisBrowser(
        lazydb::model::redis_browser::RedisBrowserTab::new(
            Uuid::from_u128(30),
            RedisTarget {
                profile_id: Uuid::from_u128(31),
                database: 0,
            },
        ),
    ));
    app.active_tab = app.tabs.len() - 1;
    app.focus = lazydb::model::workspace::Focus::Results;
    app.update(Action::FocusNext);
    assert!(
        matches!(&app.tabs[app.active_tab], WorkspaceTab::RedisBrowser(tab) if tab.focus == lazydb::model::redis_browser::RedisBrowserFocus::Preview)
    );
    app.update(Action::FocusNext);
    assert_eq!(app.focus, lazydb::model::workspace::Focus::Explorer);
    app.update(Action::FocusPrevious);
    assert!(
        matches!(&app.tabs[app.active_tab], WorkspaceTab::RedisBrowser(tab) if tab.focus == lazydb::model::redis_browser::RedisBrowserFocus::Preview)
    );
    app.update(Action::FocusPrevious);
    assert!(
        matches!(&app.tabs[app.active_tab], WorkspaceTab::RedisBrowser(tab) if tab.focus == lazydb::model::redis_browser::RedisBrowserFocus::Keys)
    );
}

#[test]
fn redis_browser_find_is_tab_local_and_edits_without_reading_a_key() {
    let mut tab = lazydb::model::redis_browser::RedisBrowserTab::new(
        Uuid::from_u128(5),
        RedisTarget {
            profile_id: Uuid::from_u128(6),
            database: 1,
        },
    );
    tab.keyspace.keys.push(RedisKeyId {
        target: tab.target.clone(),
        key: b"user:1".to_vec(),
    });
    tab.keyspace.keys.push(RedisKeyId {
        target: tab.target.clone(),
        key: b"session:2".to_vec(),
    });
    tab.rebuild_tree();
    tab.tree
        .expanded
        .insert(KeyTreeNodeId::Prefix(b"user:".to_vec()));
    tab.open_find();
    assert_eq!(tab.find.as_ref().unwrap().rows.len(), 3);
    tab.find.as_mut().unwrap().query.insert('1');
    tab.update_find();
    assert!(tab.find.as_ref().unwrap().matches.is_empty());
    tab.confirm_find();
    assert!(
        tab.find
            .as_ref()
            .unwrap()
            .matches
            .iter()
            .any(|id| *id == KeyTreeNodeId::Key(b"user:1".to_vec()))
    );
    assert_eq!(tab.preview, RedisPreviewState::Empty);
}
