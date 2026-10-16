use lazydb::{
    db::mongodb::mutation::MongoMutationOutcome,
    db::mongodb::types::MongoNamespace,
    model::{
        mongodb_browser::{MongoBrowserTab, MongoMutationKind},
        tab::WorkspaceTab,
    },
};
use uuid::Uuid;

#[test]
fn mutation_outcomes_preserve_confirmed_partial_and_unknown_certainty() {
    assert!(MongoMutationOutcome::Confirmed { affected: 4 }.is_fully_confirmed());
    assert!(
        !MongoMutationOutcome::Partial {
            affected: 2,
            requested: 5
        }
        .is_fully_confirmed()
    );
    assert!(!MongoMutationOutcome::Unknown { requested: 5 }.is_fully_confirmed());
}

#[test]
fn reducer_rejects_stale_mutation_and_does_not_refresh_uncertain_outcomes() {
    let profile_id = Uuid::new_v4();
    let tab = MongoBrowserTab::new(profile_id, MongoNamespace::collection("app", "items"));
    let tab_id = tab.id;
    let mut app = lazydb::app::App::new(Vec::new());
    app.tabs.push(WorkspaceTab::MongoBrowser(tab));
    app.active_tab = app.tabs.len() - 1;
    let first = match &mut app.tabs[app.active_tab] {
        WorkspaceTab::MongoBrowser(tab) => tab.begin_mutation(),
        _ => unreachable!(),
    };
    let second = match &mut app.tabs[app.active_tab] {
        WorkspaceTab::MongoBrowser(tab) => tab.begin_mutation(),
        _ => unreachable!(),
    };
    let commands = app.update(lazydb::action::Action::MongoMutationOutcome {
        tab_id,
        generation: 1,
        mutation_generation: first,
        operation: MongoMutationKind::Insert,
        outcome: MongoMutationOutcome::Partial {
            affected: 2,
            requested: 5,
        },
    });
    assert!(commands.is_empty());
    let commands = app.update(lazydb::action::Action::MongoMutationOutcome {
        tab_id,
        generation: 1,
        mutation_generation: second,
        operation: MongoMutationKind::Insert,
        outcome: MongoMutationOutcome::Unknown { requested: 5 },
    });
    assert!(commands.is_empty());
    let Some(WorkspaceTab::MongoBrowser(tab)) = app.tabs.last() else {
        unreachable!()
    };
    assert!(
        tab.mutation_message
            .as_deref()
            .unwrap()
            .contains("Outcome unknown")
    );
}
