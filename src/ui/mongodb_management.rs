use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::model::mongodb_management::MongoManagementDraft;

pub(crate) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    draft: &MongoManagementDraft,
    _app: &crate::app::App,
    _state: &mut super::UiState,
    theme: super::theme::Theme,
) {
    let popup = super::centered(
        area,
        area.width.saturating_sub(8).min(100),
        area.height.saturating_sub(4).max(12),
    );
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(theme.accent))
        .title(format!(" MONGODB MANAGEMENT · {:?} ", draft.operation));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(5),
        Constraint::Length(3),
    ])
    .split(inner);
    let target = draft.operation.namespace().map_or_else(|| format!("database {}", match &draft.operation { crate::model::mongodb_management::MongoManagementOperation::CreateCollection { database } | crate::model::mongodb_management::MongoManagementOperation::CreateView { database } => database, _ => "" }), |ns| ns.full_name());
    frame.render_widget(
        Paragraph::new(format!("Target: {target} · Ctrl-Enter runs, Esc cancels")),
        chunks[0],
    );
    if let Some(expected) = &draft.confirmation_text {
        let text = format!(
            "Destructive operation. Type this object name exactly:\n{expected}\n\n{}",
            draft.confirmation_input
        );
        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), chunks[1]);
    } else {
        frame.render_widget(
            Paragraph::new(draft.input.value())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Ordered JSON draft · paste JSON or append characters "),
                )
                .wrap(Wrap { trim: false }),
            chunks[1],
        );
    }
    let mut footer = vec![Line::from(Span::styled(
        "Draft is validated locally; server errors are returned as safe messages.",
        Style::new().fg(theme.muted).add_modifier(Modifier::DIM),
    ))];
    if let Some(error) = &draft.error {
        footer.push(Line::from(Span::styled(
            error.clone(),
            Style::new().fg(theme.warning),
        )));
    }
    frame.render_widget(Paragraph::new(footer).wrap(Wrap { trim: true }), chunks[2]);
}
