use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::{
    app::App,
    model::{
        mongodb_console::{MongoConsoleStatus, MongoConsoleTab},
        tab::WorkspaceTab,
    },
};

pub(crate) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    viewport: crate::model::editor::EditorViewport,
) {
    let Some(WorkspaceTab::MongoConsole(tab)) = app.tabs.get(app.active_tab) else {
        return;
    };
    let editor = app.active_mongo_console_editor_snapshot(viewport).ok();
    render_tab(frame, area, tab, editor.as_ref());
}

pub fn render_tab(
    frame: &mut Frame<'_>,
    area: Rect,
    tab: &MongoConsoleTab,
    editor: Option<&crate::model::editor::EditorRenderSnapshot>,
) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(48),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(area);
    let command = editor
        .map(|snapshot| {
            snapshot
                .lines
                .iter()
                .map(|line| line.display_text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_else(|| tab.command_text.clone());
    frame.render_widget(
        Paragraph::new(command)
            .block(
                Block::default()
                    .title(format!("Mongo JSON · {} · Ctrl+Enter run", tab.database))
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        sections[0],
    );
    let result = match &tab.status {
        MongoConsoleStatus::Idle => "Read-only Mongo commands only".to_owned(),
        MongoConsoleStatus::Running { .. } => "Running…".to_owned(),
        MongoConsoleStatus::Failed(message) => format!("Error: {message}"),
        MongoConsoleStatus::Succeeded => tab
            .result
            .as_ref()
            .map(|result| {
                let mut output = result.documents_ejson.join("\n");
                if let Some(reply) = &result.reply_ejson {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(reply);
                }
                if let Some(count) = result.count {
                    output.push_str(&format!("\nCount: {count}"));
                }
                if result.truncated {
                    output.push_str("\n… results truncated by configured budget");
                }
                if output.is_empty() {
                    "Command completed with no result documents".to_owned()
                } else {
                    output
                }
            })
            .unwrap_or_default(),
    };
    frame.render_widget(
        Paragraph::new(Text::from(
            result.lines().map(Line::from).collect::<Vec<_>>(),
        ))
        .block(Block::default().title("Result").borders(Borders::ALL))
        .wrap(Wrap { trim: false }),
        sections[1],
    );
    let color = if matches!(tab.status, MongoConsoleStatus::Failed(_)) {
        Color::Red
    } else {
        Color::Gray
    };
    frame.render_widget(
        Paragraph::new(format!(
            "Ctrl+Enter run · Alt+B/JSON begin · Alt+C/JSON commit · Alt+A/JSON abort · {:?}",
            tab.transaction
        ))
        .style(Style::default().fg(color)),
        sections[2],
    );
}
