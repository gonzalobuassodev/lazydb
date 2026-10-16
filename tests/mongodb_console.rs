use lazydb::{
    app::App,
    model::mongodb_console::{MongoConsoleStatus, MongoConsoleTab},
    persistence::mongodb_workspace::{PersistedMongoConsole, PersistedMongoWorkspace},
    profile::{CatalogScope, DatabaseKind, import_connection_url},
    ui::mongodb_console,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use uuid::Uuid;

#[test]
fn mongo_console_accepts_only_supported_read_command_and_shows_result_or_error() {
    let mut tab = MongoConsoleTab::new(Uuid::new_v4(), "app");
    assert!(tab.begin().is_ok());
    let generation = tab.generation;
    tab.complete(
        generation,
        lazydb::model::mongodb_console::MongoConsoleResult {
            documents_ejson: vec![r#"{"x":1}"#.into()],
            reply_ejson: None,
            count: None,
            truncated: false,
        },
    );
    assert_eq!(tab.status, MongoConsoleStatus::Succeeded);
    tab.set_command(r#"{"drop":"items"}"#.into());
    assert!(tab.begin().is_err());
    assert!(matches!(tab.status, MongoConsoleStatus::Idle));
}

#[test]
fn mongo_console_transaction_tracks_generation_and_unresolved_outcomes() {
    use lazydb::model::mongodb_console::MongoConsoleTransactionState as State;
    let mut tab = MongoConsoleTab::new(Uuid::new_v4(), "app");
    let first = tab.begin_transaction().unwrap();
    assert!(tab.set_transaction_result(first, State::Active));
    tab.transaction = State::Idle;
    let second = tab.begin_transaction().unwrap();
    assert!(!tab.set_transaction_result(first, State::Committed));
    assert!(tab.set_transaction_result(second, State::OutcomeUnknown));
    assert!(tab.begin_transaction().is_err());
}

#[test]
fn mongo_console_transaction_control_requires_a_single_explicit_field() {
    use lazydb::model::mongodb_console::{MongoTransactionControl, parse_transaction_control};
    assert_eq!(
        parse_transaction_control(r#"{"$transaction":"begin"}"#).unwrap(),
        Some(MongoTransactionControl::Begin)
    );
    assert_eq!(
        parse_transaction_control(r#"{"$transaction":"commit"}"#).unwrap(),
        Some(MongoTransactionControl::Commit)
    );
    assert!(parse_transaction_control(r#"{"$transaction":"abort","$db":"admin"}"#).is_err());
    assert!(parse_transaction_control(r#"{"$transaction":"drop"}"#).is_err());
}

#[test]
fn mongo_console_render_shows_json_input_results_and_errors() {
    let mut tab = MongoConsoleTab::new(Uuid::new_v4(), "app");
    let generation = tab.begin().unwrap();
    tab.complete(
        generation,
        lazydb::model::mongodb_console::MongoConsoleResult {
            documents_ejson: vec![r#"{"ok":1}"#.into()],
            reply_ejson: None,
            count: None,
            truncated: true,
        },
    );
    let backend = TestBackend::new(90, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| mongodb_console::render_tab(frame, Rect::new(0, 0, 90, 24), &tab, None))
        .unwrap();
    let text = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(text.contains("Mongo JSON"));
    assert!(text.contains("truncated"));
}

#[test]
fn restored_console_is_idle_with_static_command_and_never_schedules_execution() {
    let mut profile = import_connection_url("mongodb://localhost:27017/app", Some("mongo"))
        .unwrap()
        .profile;
    profile.kind = DatabaseKind::MongoDb;
    profile.catalog_scope = CatalogScope::for_profile(DatabaseKind::MongoDb, "app", None);
    let profile_id = profile.id;
    let mut app = App::new(vec![profile]);
    let id = Uuid::new_v4();
    app.restore_mongo_workspace(PersistedMongoWorkspace {
        consoles: vec![PersistedMongoConsole {
            id,
            profile_id,
            database: "app".into(),
            command_file: format!("{id}.mongodb.json").into(),
            open: true,
        }],
        commands: vec![(id, r#"{"count":"items"}"#.into())],
    });
    let Some(lazydb::model::tab::WorkspaceTab::MongoConsole(tab)) = app.tabs.last() else {
        panic!("Mongo console should restore as a tab");
    };
    assert_eq!(tab.command_text, r#"{"count":"items"}"#);
    assert_eq!(tab.status, MongoConsoleStatus::Idle);
    assert!(tab.result.is_none());
}
