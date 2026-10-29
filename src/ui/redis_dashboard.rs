use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use super::{HitRegion, HitTarget, Theme, UiState, panel_block};
use crate::{app::App, model::tab::WorkspaceTab};

fn configured_name(app: &App, profile_id: Option<uuid::Uuid>) -> &'static str {
    profile_id
        .and_then(|profile_id| app.profiles.iter().find(|profile| profile.id == profile_id))
        .map(|profile| match profile.kind {
            crate::profile::DatabaseKind::Valkey => "Valkey",
            _ => "Redis",
        })
        .unwrap_or("Redis-compatible")
}

fn observed_identity(
    details: &crate::db::redis::monitor::RedisMonitorDetails,
    configured_name: &'static str,
) -> String {
    let identity = &details.server_identity;
    format!(
        "{} {} · {} · {}",
        identity.display_name_or(configured_name),
        identity.version.as_deref().unwrap_or("--"),
        identity.mode.as_deref().unwrap_or("--"),
        details
            .uptime_in_seconds
            .map(format_uptime)
            .unwrap_or_else(|| "uptime --".into())
    )
}

pub(crate) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    theme: Theme,
    state: &mut UiState,
) {
    let Some(WorkspaceTab::Dashboard(tab)) = app.tabs.get(app.active_tab) else {
        return;
    };
    let configured_name = configured_name(
        app,
        tab.connection
            .map(|connection| connection.profile_id)
            .or(tab.profile_id),
    );
    let title = format!(" {configured_name} DASHBOARD ");
    let block = panel_block(&title, true, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }
    let details = tab.redis_details.as_ref();
    if tab.page == crate::model::dashboard::DashboardPage::Info {
        render_info(frame, inner, theme, details, app.redis_info_scroll);
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Min(1),
        ])
        .split(inner);
    let identity = details
        .map(|details| observed_identity(details, configured_name))
        .unwrap_or_else(|| format!("{configured_name} · waiting for INFO"));
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(identity, theme.title(true)),
            Line::from(format!(
                "Instance scope · context db{}",
                tab.redis_database.unwrap_or(0)
            )),
        ]),
        rows[0],
    );
    let cards = [
        (
            "OPS/s",
            value(tab, crate::model::dashboard::MetricKey::RedisCommands),
            theme.accent,
        ),
        (
            "Clients",
            value(tab, crate::model::dashboard::MetricKey::Connections),
            theme.action,
        ),
        (
            "Memory",
            format_bytes(value_number(
                tab,
                crate::model::dashboard::MetricKey::RedisMemory,
            )),
            theme.warning,
        ),
        (
            "Keys",
            value(tab, crate::model::dashboard::MetricKey::RedisKeys),
            theme.success,
        ),
    ];
    render_cards(frame, rows[1], theme, &cards);
    let status = details
        .map(|details| {
            format!(
                "Port {} · PID {} · fragmentation {} · AOF {}",
                details
                    .tcp_port
                    .map_or_else(|| "--".into(), |value| value.to_string()),
                details
                    .process_id
                    .map_or_else(|| "--".into(), |value| value.to_string()),
                details
                    .mem_fragmentation_ratio
                    .map_or_else(|| "--".into(), |value| format!("{value:.2}")),
                if details.aof_enabled == Some(true) {
                    "enabled"
                } else {
                    "disabled"
                }
            )
        })
        .unwrap_or_else(|| "Server information unavailable".into());
    frame.render_widget(
        Paragraph::new(status).style(Style::new().fg(theme.muted)),
        rows[2],
    );
    super::dashboard::render_metric_chart(
        frame,
        rows[3],
        theme,
        tab,
        " Commands/s · last 10 minutes ",
        &[(
            crate::model::dashboard::MetricKey::RedisCommands,
            "ops/s",
            theme.accent,
        )],
    );
    let info = details
        .map(|details| {
            details
                .fields
                .iter()
                .flat_map(|(section, fields)| {
                    fields
                        .iter()
                        .map(move |(key, value)| format!("{section}.{key}: {value}"))
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_else(|| format!("{configured_name} INFO is not available"));
    state.hit_regions.push(HitRegion {
        area: rows[3],
        target: HitTarget::OpenTextDetail(super::readonly_detail_request(
            format!("{configured_name} INFO"),
            info,
        )),
    });
}

fn render_info(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: Theme,
    details: Option<&crate::db::redis::monitor::RedisMonitorDetails>,
    scroll: u16,
) {
    let Some(details) = details else {
        frame.render_widget(
            Paragraph::new("Waiting for key-value server INFO…")
                .style(Style::new().fg(theme.muted)),
            area,
        );
        return;
    };
    let mut lines = Vec::new();
    for (section, fields) in &details.fields {
        lines.push(Line::styled(format!("[{section}]"), theme.title(true)));
        for (key, value) in fields {
            lines.push(Line::from(vec![
                Span::styled(format!("{key:<34}"), Style::new().fg(theme.muted)),
                Span::raw(value.clone()),
            ]));
        }
        lines.push(Line::raw(""));
    }
    let max_scroll = lines.len().saturating_sub(area.height as usize) as u16;
    let scroll = scroll.min(max_scroll);
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), area);
}

fn render_cards(
    frame: &mut Frame<'_>,
    area: Rect,
    theme: Theme,
    cards: &[(&str, String, ratatui::style::Color)],
) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(25); 4])
        .split(area);
    for (index, (label, value, color)) in cards.iter().enumerate() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(*label, Style::new().fg(theme.muted))),
                Line::raw(""),
                Line::styled(value, Style::new().fg(*color)),
            ])
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(theme.border)),
            ),
            columns[index],
        );
    }
}

fn value(
    tab: &crate::model::dashboard::DashboardTab,
    key: crate::model::dashboard::MetricKey,
) -> String {
    value_number(tab, key).map_or_else(|| "--".into(), format_number)
}
fn value_number(
    tab: &crate::model::dashboard::DashboardTab,
    key: crate::model::dashboard::MetricKey,
) -> Option<f64> {
    tab.latest
        .as_ref()
        .and_then(|sample| sample.values.get(&key))
        .copied()
}
fn format_number(value: f64) -> String {
    if value.fract() == 0.0 {
        (value as u64).to_string()
    } else {
        format!("{value:.1}")
    }
}
fn format_bytes(value: Option<f64>) -> String {
    value.map_or_else(
        || "--".into(),
        |value| {
            let mut value = value;
            let units = ["B", "KiB", "MiB", "GiB"];
            let mut index = 0;
            while value >= 1024.0 && index < units.len() - 1 {
                value /= 1024.0;
                index += 1;
            }
            format!("{value:.1} {}", units[index])
        },
    )
}

fn format_uptime(seconds: u64) -> String {
    format!(
        "uptime {}d {:02}:{:02}:{:02}",
        seconds / 86400,
        (seconds / 3600) % 24,
        (seconds / 60) % 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::{configured_name, observed_identity};
    use crate::{app::App, db::redis::monitor::parse_info, profile::import_connection_url};

    #[test]
    fn dashboard_configuration_name_comes_from_its_bound_profile() {
        let redis = import_connection_url("redis://localhost", Some("redis"))
            .unwrap()
            .profile;
        let valkey = import_connection_url("valkey://localhost", Some("valkey"))
            .unwrap()
            .profile;
        let redis_id = redis.id;
        let valkey_id = valkey.id;
        let app = App::new(vec![redis, valkey]);

        assert_eq!(configured_name(&app, Some(redis_id)), "Redis");
        assert_eq!(configured_name(&app, Some(valkey_id)), "Valkey");
        assert_eq!(configured_name(&app, None), "Redis-compatible");
        assert_eq!(
            configured_name(&app, Some(uuid::Uuid::nil())),
            "Redis-compatible"
        );
    }

    #[test]
    fn dashboard_summary_uses_observed_brand_and_actual_version() {
        let (_, details) = parse_info(
            "# Server\nredis_version:7.2.4\nserver_name:valkey\nvalkey_version:8.1.3\nredis_mode:standalone\nserver_mode:cluster\n# Server\nuptime_in_seconds:10\n",
            1,
        );
        let summary = observed_identity(&details, "Valkey");
        assert!(summary.starts_with("Valkey 8.1.3 · cluster ·"));
        assert!(!summary.contains("7.2.4"));
    }

    #[test]
    fn dashboard_summary_does_not_use_redis_compatibility_version_for_valkey() {
        let (_, details) = parse_info("# Server\nredis_version:7.2.4\nserver_name:valkey\n", 1);
        let summary = observed_identity(&details, "Valkey");
        assert!(summary.starts_with("Valkey -- · -- ·"));
    }
}
