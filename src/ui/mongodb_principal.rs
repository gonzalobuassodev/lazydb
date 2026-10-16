use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    database: &str,
    users: &[crate::db::mongodb::principal::MongoPrincipalInfo],
    message: Option<&str>,
) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(4)])
        .split(area);
    frame.render_widget(
        Paragraph::new(format!("Users & Roles · {database} · r to refresh"))
            .block(Block::default().borders(Borders::ALL)),
        areas[0],
    );
    let content = users
        .iter()
        .map(|user| {
            format!(
                "{}@{}  roles: {}",
                user.name,
                user.database,
                user.roles.len()
            )
        })
        .chain(message.into_iter().map(str::to_owned))
        .collect::<Vec<_>>()
        .join("\n");
    frame.render_widget(
        Paragraph::new(content)
            .block(
                Block::default()
                    .title("MongoDB principals · details are permission scoped")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        areas[1],
    );
}

pub(crate) fn render_mutation(
    frame: &mut Frame<'_>,
    area: Rect,
    draft: &crate::model::mongodb_principal::MongoPrincipalMutationDraft,
    theme: super::theme::Theme,
    state: &mut super::UiState,
) {
    let popup = super::centered(area, area.width.min(92), area.height.clamp(12, 28));
    frame.render_widget(Clear, popup);
    let inner = Block::default()
        .title(" MONGODB PRINCIPAL · EPHEMERAL INPUT ")
        .borders(Borders::ALL)
        .border_style(Style::new().fg(theme.accent))
        .inner(popup);
    frame.render_widget(
        Block::default()
            .title(" MONGODB PRINCIPAL · EPHEMERAL INPUT ")
            .borders(Borders::ALL)
            .border_style(Style::new().fg(theme.accent)),
        popup,
    );
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);
    let password_mask = "•".repeat(draft.password_value().chars().count());
    let values = [
        ("Operation", draft.display_operation()),
        ("Target", draft.display_target()),
        ("Password", password_mask.as_str()),
        ("Roles JSON", draft.roles_json_value()),
        ("Privileges JSON", draft.privileges_json_value()),
    ];
    let cursor = draft.field_cursor(draft.selected_field);
    for (index, (label, value)) in values.into_iter().enumerate() {
        let selected = matches!(
            (index, draft.selected_field),
            (
                0,
                crate::model::mongodb_principal::MongoPrincipalField::Operation
            ) | (
                1,
                crate::model::mongodb_principal::MongoPrincipalField::Target
            ) | (
                2,
                crate::model::mongodb_principal::MongoPrincipalField::Password
            ) | (
                3,
                crate::model::mongodb_principal::MongoPrincipalField::Roles
            ) | (
                4,
                crate::model::mongodb_principal::MongoPrincipalField::Privileges
            )
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{label}: "), theme.title(false)),
                Span::raw(value),
            ]))
            .style(if selected {
                theme.title(true).add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            })
            .wrap(Wrap { trim: false }),
            rows[index],
        );
        if selected {
            let prefix_width = format!("{label}: ").chars().count() as u16;
            state.cursor = Some(super::CursorSpec {
                position: ratatui::layout::Position::new(
                    rows[index]
                        .x
                        .saturating_add(prefix_width)
                        .saturating_add(cursor as u16),
                    rows[index].y,
                ),
                style: super::CursorStyle::Bar,
            });
        }
    }
    frame.render_widget(
        Paragraph::new("Tab fields · ↑/↓ operation · Enter review · Esc cancel"),
        rows[5],
    );
    let selected_index = match draft.selected_field {
        crate::model::mongodb_principal::MongoPrincipalField::Operation => 0,
        crate::model::mongodb_principal::MongoPrincipalField::Target => 1,
        crate::model::mongodb_principal::MongoPrincipalField::Password => 2,
        crate::model::mongodb_principal::MongoPrincipalField::Roles => 3,
        crate::model::mongodb_principal::MongoPrincipalField::Privileges => 4,
    };
    let label_width = [
        "Operation: ",
        "Target: ",
        "Password: ",
        "Roles JSON: ",
        "Privileges JSON: ",
    ][selected_index]
        .chars()
        .count() as u16;
    state.cursor = Some(super::CursorSpec {
        position: ratatui::layout::Position::new(
            rows[selected_index]
                .x
                .saturating_add(label_width)
                .saturating_add(draft.selected_field_cursor() as u16),
            rows[selected_index].y,
        ),
        style: super::CursorStyle::Bar,
    });
}

pub(crate) fn render_confirmation(
    frame: &mut Frame<'_>,
    area: Rect,
    mutation: &crate::db::mongodb::principal::MongoPrincipalMutation,
    expected: &str,
    input: &str,
    theme: super::theme::Theme,
    state: &mut super::UiState,
) {
    let popup = super::centered(area, area.width.min(86), area.height.clamp(9, 16));
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .title(" CONFIRM MONGODB PRINCIPAL CHANGE ")
        .borders(Borders::ALL)
        .border_style(Style::new().fg(theme.warning));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let destructive = matches!(mutation.command_name(), Some("dropUser" | "dropRole"));
    let lines = vec![
        Line::from(format!(
            "{} target {}",
            mutation.command_name().unwrap_or("operation"),
            expected
        )),
        Line::from(if destructive {
            "Type the exact database:target below to continue:"
        } else {
            "Review this principal operation. Type the target to confirm:"
        }),
        Line::from(format!("Confirmation: {input}")),
        Line::from("Enter applies only after the exact target matches. Esc cancels."),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    state.cursor = Some(super::CursorSpec {
        position: ratatui::layout::Position::new(
            inner.x.saturating_add(15 + input.chars().count() as u16),
            inner.y.saturating_add(2),
        ),
        style: super::CursorStyle::Bar,
    });
}
