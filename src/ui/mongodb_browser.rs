use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

use crate::model::{mongodb_browser::MongoPageState, tab::WorkspaceTab};

pub(crate) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &crate::app::App,
    theme: crate::ui::theme::Theme,
    ui: &mut crate::ui::UiState,
) {
    let Some(WorkspaceTab::MongoBrowser(tab)) = app.tabs.get(app.active_tab) else {
        return;
    };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(44), Constraint::Percentage(56)])
        .split(area);
    let rows: Vec<ListItem<'_>> = match &tab.page {
        MongoPageState::Empty => vec![ListItem::new("Choose a collection to load documents")],
        MongoPageState::Loading => vec![ListItem::new("Loading documents…")],
        MongoPageState::Failed(error) => vec![ListItem::new(format!("Load failed: {error}"))],
        MongoPageState::Ready(page) if page.documents.is_empty() => {
            vec![ListItem::new("No documents")]
        }
        MongoPageState::Ready(page) => page
            .documents
            .iter()
            .enumerate()
            .map(|(index, document)| {
                let id = document
                    .bson
                    .get("_id")
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "(missing _id)".to_owned());
                let label = format!("{}  {}", index + 1, id);
                let item = ListItem::new(label);
                if index == tab.selected_document {
                    item.style(Style::new().bg(theme.selection))
                } else {
                    item
                }
            })
            .collect(),
    };
    frame.render_widget(
        List::new(rows).block(Block::default().title("Documents").borders(Borders::ALL)),
        columns[0],
    );
    let detail = if tab.query_field.is_some() {
        format!(
            "{}{}",
            tab.query_text,
            tab.query_error
                .as_ref()
                .map(|error| format!("\n\nError: {error}"))
                .unwrap_or_default()
        )
    } else if tab.editing {
        format!(
            "{}{}",
            tab.edit_text,
            tab.edit_error
                .as_ref()
                .map(|error| format!("\n\nError: {error}"))
                .unwrap_or_default()
        )
    } else if let Some(draft) = &tab.import_draft {
        format!(
            "Import draft · {} document(s)\n{}",
            draft.documents.len(),
            draft.documents.join("\n")
        )
    } else {
        tab.selected_document_inspector().map(|(text, _)| text).unwrap_or_else(|| match &tab.page {
        MongoPageState::Ready(page) if page.oversized_document => "Selected document exceeds the configured byte budget. Raw BSON is retained by the reader.".to_owned(),
        MongoPageState::Ready(page) if page.truncated => "Page reached the configured byte budget. Load the next page or reduce the result size.".to_owned(),
        _ => "Select a document to inspect Canonical Extended JSON.".to_owned(),
    })
    };
    let status = match &tab.page {
        MongoPageState::Ready(page) => format!(
            "{} documents · {} BSON bytes{}",
            page.documents.len(),
            page.bytes,
            if page.truncated { " · truncated" } else { "" }
        ),
        MongoPageState::Failed(error) => format!("{} · press r to retry", error),
        _ => tab
            .namespace
            .collection
            .as_deref()
            .unwrap_or(&tab.namespace.database)
            .to_owned(),
    };
    let status = tab.mutation_message.as_deref().unwrap_or(&status);
    let status_style =
        if status.contains("unknown") || status.contains("Partial") || status.contains("partial") {
            Style::new().fg(ratatui::style::Color::Yellow)
        } else {
            Style::new().fg(theme.muted)
        };
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(columns[1]);
    let title = if let Some(query_field) = tab.query_field {
        format!("Edit {query_field:?} · Enter apply · Esc cancel")
    } else if tab.editing {
        "Edit Canonical EJSON · Ctrl+Enter save · Esc cancel".to_owned()
    } else {
        if tab
            .selected_document_inspector()
            .is_some_and(|(_, raw_only)| raw_only)
        {
            "Raw BSON inspector · read only".to_owned()
        } else {
            "Canonical EJSON · e edit · y copy".to_owned()
        }
    };
    frame.render_widget(
        Paragraph::new(Text::from(
            detail.lines().map(Line::from).collect::<Vec<_>>(),
        ))
        .block(Block::default().title(title).borders(Borders::ALL))
        .wrap(ratatui::widgets::Wrap { trim: false }),
        right[0],
    );
    if let MongoPageState::Ready(page) = &tab.page
        && page.next_offset.is_some()
    {
        let more = Rect::new(
            columns[0].x + 1,
            columns[0].y + columns[0].height.saturating_sub(1),
            columns[0].width.saturating_sub(2),
            1,
        );
        frame.render_widget(
            Paragraph::new("Load next page").style(Style::new().fg(theme.accent)),
            more,
        );
        ui.hit_regions.push(crate::ui::HitRegion {
            area: more,
            target: crate::ui::HitTarget::MongoPageLoadMore(tab.id),
        });
    }
    let controls = Rect::new(
        columns[0].x,
        columns[0].y + columns[0].height.saturating_sub(2),
        columns[0].width,
        1,
    );
    let control_text = if tab.import_draft.is_some() {
        "Import draft ready · I insert · Esc discard"
    } else {
        "paste import · i insert · d delete · y copy · E export loaded EJSON"
    };
    frame.render_widget(
        Paragraph::new(control_text).style(Style::new().fg(theme.accent)),
        controls,
    );
    let labels = [
        (
            "Filter",
            crate::model::mongodb_browser::MongoQueryField::Filter,
        ),
        ("Sort", crate::model::mongodb_browser::MongoQueryField::Sort),
        (
            "Projection",
            crate::model::mongodb_browser::MongoQueryField::Projection,
        ),
    ];
    let width = (area.width / 3).max(1);
    for (index, (label, field)) in labels.into_iter().enumerate() {
        let rect = Rect::new(
            area.x + (index as u16 * width),
            area.y,
            if index == 2 {
                area.width.saturating_sub(width * 2)
            } else {
                width
            },
            1,
        );
        let text = match field {
            crate::model::mongodb_browser::MongoQueryField::Filter => tab.spec.filter.to_string(),
            crate::model::mongodb_browser::MongoQueryField::Sort => tab
                .spec
                .sort
                .as_ref()
                .map_or("{}".into(), ToString::to_string),
            crate::model::mongodb_browser::MongoQueryField::Projection => tab
                .spec
                .projection
                .as_ref()
                .map_or("{}".into(), ToString::to_string),
        };
        frame.render_widget(
            Paragraph::new(format!("{label}: {text}")).style(Style::new().fg(theme.muted)),
            rect,
        );
        ui.hit_regions.push(crate::ui::HitRegion {
            area: rect,
            target: crate::ui::HitTarget::MongoQueryField(tab.id, field),
        });
    }
    frame.render_widget(Paragraph::new(status).style(status_style), right[1]);
}
