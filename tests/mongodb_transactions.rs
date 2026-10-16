use lazydb::{
    action::{Action, Command},
    app::App,
    model::{
        mongodb_console::{MongoConsoleTab, MongoConsoleTransactionState as State},
        tab::WorkspaceTab,
        transaction::TransactionExitChoice,
    },
    profile::{DatabaseKind, import_connection_url},
};
use uuid::Uuid;

fn app_with_mongo_console() -> (App, Uuid) {
    let mut profile = import_connection_url("mongodb://localhost:27017/app", Some("mongo"))
        .unwrap()
        .profile;
    profile.kind = DatabaseKind::MongoDb;
    let profile_id = profile.id;
    let mut app = App::new(vec![profile]);
    let mut tab = MongoConsoleTab::new(profile_id, "app");
    let id = tab.id;
    tab.transaction = State::Active;
    tab.transaction_generation = 1;
    app.tabs.push(WorkspaceTab::MongoConsole(tab));
    (app, id)
}

#[test]
fn closing_mongo_tab_defers_until_transaction_rollback_finishes() {
    let (mut app, id) = app_with_mongo_console();
    assert!(app.update(Action::CloseTab(id)).is_empty());
    assert!(app.overlay.is_some());
    app.update(Action::ToggleTransactionExitChoice);
    let commands = app.update(Action::ConfirmTransactionExit);
    assert!(commands.iter().any(|command| matches!(command, Command::MongoTransactionAbort { tab_id, .. } if *tab_id == id)), "commands={commands:?}, overlay={:?}", app.overlay);
    let generation = match app.tabs.iter().find(|tab| tab.id() == id).unwrap() {
        WorkspaceTab::MongoConsole(tab) => tab.transaction_generation,
        _ => unreachable!(),
    };
    assert!(generation > 0);
    app.update(Action::MongoTransactionFinished {
        tab_id: id,
        generation,
        outcome: lazydb::db::mongodb::transaction::MongoTransactionOutcome::Aborted,
    });
    assert!(app.tabs.iter().all(|tab| tab.id() != id));
}

#[test]
fn switching_tabs_prompts_for_active_mongo_transaction() {
    let (mut app, _) = app_with_mongo_console();
    app.tabs
        .push(WorkspaceTab::MongoConsole(MongoConsoleTab::new(
            Uuid::new_v4(),
            "other",
        )));
    app.active_tab = 0;
    assert!(app.update(Action::ActivateTab(1)).is_empty());
    assert!(app.overlay.is_some());
    app.update(Action::ConfirmTransactionExitChoice(
        TransactionExitChoice::Cancel,
    ));
    assert_eq!(app.active_tab, 0);
}
