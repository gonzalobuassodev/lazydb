use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    widgets::{Block, Borders, Paragraph, Wrap},
};

pub fn render(frame: &mut Frame<'_>, area: Rect, tab: &crate::model::dashboard::MongoDashboardTab) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(3),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new(format!(
            "MongoDB monitor · {} · sampled {:?}",
            tab.database,
            tab.snapshot.as_ref().map(|snapshot| snapshot.sampled_at)
        ))
        .block(Block::default().title("Overview").borders(Borders::ALL)),
        chunks[0],
    );
    let mut body = Vec::new();
    if let Some(snapshot) = &tab.snapshot {
        body.push(format!(
            "serverStatus: {}",
            if snapshot.server_status.is_some() {
                "available"
            } else {
                "unavailable"
            }
        ));
        body.push(format!(
            "dbStats: {}",
            if snapshot.database_stats.is_some() {
                "available"
            } else {
                "unavailable"
            }
        ));
        body.push(format!(
            "currentOp rows: {}",
            snapshot.current_operations.len()
        ));
    }
    body.extend(tab.errors.iter().cloned());
    body.extend(tab.users.iter().map(|user| {
        format!(
            "user {}@{} · {} roles",
            user.name,
            user.database,
            user.roles.len()
        )
    }));
    body.extend(tab.roles.iter().map(|role| {
        format!(
            "role {}@{} · {} privileges",
            role.get_str("role").unwrap_or("unknown"),
            role.get_str("db").unwrap_or("unknown"),
            role.get_array("privileges").map_or(0, Vec::len)
        )
    }));
    if tab.loading {
        body.push("Sampling…".into());
    }
    frame.render_widget(
        Paragraph::new(body.join("\n"))
            .block(
                Block::default()
                    .title("Mongo metrics and permission details")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        chunks[1],
    );
    frame.render_widget(
        Paragraph::new("r refresh · u users/roles · n new user · N new role; operation field cycles all user/role changes")
            .block(Block::default().borders(Borders::ALL)),
        chunks[2],
    );
}
