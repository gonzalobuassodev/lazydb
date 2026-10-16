use lazydb::{
    action::Action,
    app::App,
    model::{
        mongodb_console::{MongoConsoleTab, MongoConsoleTransactionState as State},
        tab::WorkspaceTab,
        transaction::TransactionExitChoice,
    },
    profile::{DatabaseKind, import_connection_url},
};

#[test]
fn quitting_with_unknown_mongo_transaction_requires_explicit_abandon() {
    let mut profile = import_connection_url("mongodb://localhost:27017/app", Some("mongo"))
        .unwrap()
        .profile;
    profile.kind = DatabaseKind::MongoDb;
    let mut app = App::new(vec![profile.clone()]);
    let mut tab = MongoConsoleTab::new(profile.id, "app");
    let id = tab.id;
    let generation = tab.begin_transaction().unwrap();
    assert!(tab.set_transaction_result(generation, State::OutcomeUnknown));
    app.tabs.push(WorkspaceTab::MongoConsole(tab));
    assert!(app.update(Action::Quit).is_empty());
    assert!(app.overlay.is_some());
    app.update(Action::ConfirmTransactionExitChoice(
        TransactionExitChoice::Abandon,
    ));
    assert!(app.should_quit || app.tabs.iter().any(|tab| tab.id() == id));
}
