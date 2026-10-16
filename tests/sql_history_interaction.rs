use lazydb::{
    action::{Action, Command},
    app::App,
    clipboard::ClipboardPayload,
    model::{sql_history::*, sql_history_view::SqlHistoryState, workspace::Overlay},
};
use uuid::Uuid;

fn app_with_history(sql: &str) -> App {
    let mut app = App::new(Vec::new());
    let id = Uuid::new_v4();
    app.overlay = Some(Overlay::SqlHistory(SqlHistoryState {
        selected_execution: Some(id),
        items: vec![ExecutionHistory {
            execution_id: id,
            operation_id: Uuid::new_v4(),
            transaction_id: None,
            sql: sql.into(),
            status: HistoryExecutionStatus::Succeeded,
            certainty: HistoryResultCertainty::Confirmed,
            transaction_outcome: HistoryTransactionOutcome::AutoCommitted,
            affected_rows: Some(2),
            returned_rows: None,
            requested_at: 0,
            elapsed_millis: Some(4),
            profile_id: None,
            database: None,
            schema: None,
            language: lazydb::model::sql_history::HistoryLanguage::Sql,
            mongo_command: None,
        }],
        ..Default::default()
    }));
    app
}

#[test]
fn history_copy_action_uses_complete_sql_not_a_preview() {
    let sql = "SELECT * FROM users\nWHERE display_name = '中文'";
    let mut app = app_with_history(sql);

    let commands = app.update(Action::SqlHistoryCopy);

    assert!(matches!(
        commands.as_slice(),
        [Command::WriteClipboard(ClipboardPayload { text, .. })] if text == sql
    ));
}

#[test]
fn history_detail_action_opens_existing_complete_text_detail() {
    let sql = "SELECT * FROM users\nWHERE id = 1";
    let mut app = app_with_history(sql);

    app.update(Action::SqlHistoryOpenDetail);

    let Overlay::SqlHistory(view) = app.overlay.unwrap() else {
        panic!("expected SQL history overlay");
    };
    assert_eq!(
        view.mode,
        lazydb::model::sql_history_view::SqlHistoryMode::Sql
    );
    assert_ne!(view.editor_session_id, Uuid::nil());
}

#[test]
fn opening_history_uses_an_overlay_without_creating_a_workspace_tab() {
    let mut app = App::new(Vec::new());
    let tab_count = app.tabs.len();

    let commands = app.update(Action::OpenSqlHistory);

    assert_eq!(app.tabs.len(), tab_count);
    assert!(matches!(app.overlay, Some(Overlay::SqlHistory(_))));
    assert!(matches!(
        commands.as_slice(),
        [Command::LoadSqlHistory { overlay_id, .. }]
            if *overlay_id != Uuid::nil()
    ));
}

#[test]
fn dismissing_history_closes_only_the_history_overlay() {
    let mut app = App::new(Vec::new());
    app.update(Action::OpenSqlHistory);

    assert!(app.update(Action::DismissOverlay).is_empty());
    assert_eq!(app.overlay, None);
}
