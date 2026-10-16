use std::collections::BTreeMap;

use crate::{
    db::{
        DatabaseError, ErrorCategory,
        monitor::{MAX_PROCESS_ROWS, MonitorSnapshot, MonitorVisibility, ProcessSnapshot},
        query::QueryBudget,
    },
    model::dashboard::{MetricKey, ProcessRow},
    security::sanitize_terminal_text,
};

use super::transport::ClickHouseAdapter;

impl ClickHouseAdapter {
    pub async fn load_monitor_snapshot(&self) -> Result<MonitorSnapshot, DatabaseError> {
        let sql = "SELECT event, value FROM system.events WHERE event IN ('Query', 'SelectQuery', 'InsertQuery') ORDER BY event";
        let result = self
            .execute_with_budget(
                sql,
                QueryBudget {
                    max_rows: 64,
                    max_bytes: 256 * 1024,
                },
            )
            .await
            .map_err(restricted_monitor_error)?;
        let names = result
            .result_sets
            .first()
            .map(|set| {
                set.columns
                    .iter()
                    .map(|column| column.name.as_str())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let event_index = names.iter().position(|name| *name == "event");
        let value_index = names.iter().position(|name| *name == "value");
        let mut counters = BTreeMap::<String, f64>::new();
        if let (Some(event_index), Some(value_index)) = (event_index, value_index)
            && let Some(set) = result.result_sets.first()
        {
            for row in &set.rows {
                let (Some(crate::db::value::CellValue::Text(event)), Some(value)) =
                    (row.get(event_index), row.get(value_index))
                else {
                    continue;
                };
                let number = match value {
                    crate::db::value::CellValue::Unsigned(value) if *value <= (1u64 << 53) => {
                        *value as f64
                    }
                    crate::db::value::CellValue::Integer(value)
                        if *value >= 0 && (*value as u64) <= (1u64 << 53) =>
                    {
                        *value as f64
                    }
                    crate::db::value::CellValue::Float(value) if value.is_finite() => *value,
                    _ => continue,
                };
                counters.insert(event.clone(), number);
            }
        }
        let mut values = BTreeMap::new();
        for (event, metric) in [
            ("Query", MetricKey::Queries),
            ("SelectQuery", MetricKey::Selects),
            ("InsertQuery", MetricKey::Inserts),
        ] {
            if let Some(value) = counters.get(event) {
                values.insert(metric, *value);
            }
        }
        Ok(MonitorSnapshot {
            server_time_millis: now_millis(),
            server_generation: self.profile_id().as_u128() as u64,
            values,
            redis_details: None,
            visibility: MonitorVisibility::Unknown,
        })
    }

    pub async fn load_process_snapshot(&self) -> Result<ProcessSnapshot, DatabaseError> {
        let sql = format!(
            "SELECT query_id, user, initial_user, address, initial_address, current_database, query, elapsed, is_initial_query FROM system.processes WHERE is_initial_query = 1 ORDER BY elapsed DESC LIMIT {}",
            MAX_PROCESS_ROWS + 1
        );
        let result = self
            .execute_with_budget(
                &sql,
                QueryBudget {
                    max_rows: MAX_PROCESS_ROWS + 1,
                    max_bytes: 16 * 1024 * 1024,
                },
            )
            .await
            .map_err(restricted_monitor_error)?;
        let budget_truncated = result.stats.truncated;
        let set = result.result_sets.first();
        let Some(set) = set else {
            return Ok(ProcessSnapshot {
                rows: Vec::new(),
                truncated: false,
                visibility: MonitorVisibility::Unknown,
            });
        };
        let index = |name: &str| set.columns.iter().position(|column| column.name == name);
        let query_id = required_index(&set.columns, "query_id")?;
        let user_index = index("user");
        let initial_user = index("initial_user");
        let address = index("address");
        let initial_address = index("initial_address");
        let database = index("current_database");
        let query = index("query");
        let elapsed = index("elapsed");
        let initial = index("is_initial_query");
        let mut rows = Vec::with_capacity(set.rows.len().min(MAX_PROCESS_ROWS));
        for row in &set.rows {
            let id = text_value(row.get(query_id)).ok_or_else(|| decode_error("query_id"))?;
            let is_initial = initial
                .and_then(|index| bool_value(row.get(index)))
                .unwrap_or(true);
            if !is_initial {
                continue;
            }
            rows.push(ProcessRow {
                id: 0,
                native_id: Some(sanitize_terminal_text(id)),
                user: sanitize_terminal_text(
                    initial_user
                        .or(user_index)
                        .and_then(|index| text_value(row.get(index)))
                        .unwrap_or_default(),
                ),
                database: database
                    .and_then(|index| optional_text(row.get(index)))
                    .map(|value| sanitize_terminal_text(&value)),
                client: initial_address
                    .or(address)
                    .and_then(|index| optional_text(row.get(index)))
                    .map(|value| sanitize_terminal_text(&value)),
                application: None,
                state: Some("running".into()),
                wait: None,
                elapsed: std::time::Duration::from_secs_f64(
                    elapsed
                        .and_then(|index| number_value(row.get(index)))
                        .unwrap_or_default()
                        .clamp(0.0, 1.0e12),
                ),
                query: query
                    .and_then(|index| optional_text(row.get(index)))
                    .map(|value| sanitize_terminal_text(&value)),
            });
        }
        let truncated = rows.len() > MAX_PROCESS_ROWS || budget_truncated;
        rows.truncate(MAX_PROCESS_ROWS);
        Ok(ProcessSnapshot {
            rows,
            truncated,
            // ClickHouse grants can restrict the visible process rows without
            // exposing a completeness flag. Do not infer visibility from the
            // absence of an HTTP permission error.
            visibility: MonitorVisibility::Unknown,
        })
    }
}

fn required_index(
    columns: &[crate::db::query::ColumnMeta],
    name: &str,
) -> Result<usize, DatabaseError> {
    columns
        .iter()
        .position(|column| column.name == name)
        .ok_or_else(|| decode_error(name))
}

fn text_value(value: Option<&crate::db::value::CellValue>) -> Option<&str> {
    match value {
        Some(crate::db::value::CellValue::Text(value)) => Some(value),
        _ => None,
    }
}

fn optional_text(value: Option<&crate::db::value::CellValue>) -> Option<String> {
    match value {
        Some(crate::db::value::CellValue::Text(value)) if !value.is_empty() => Some(value.clone()),
        _ => None,
    }
}

fn number_value(value: Option<&crate::db::value::CellValue>) -> Option<f64> {
    match value {
        Some(crate::db::value::CellValue::Unsigned(value)) => Some(*value as f64),
        Some(crate::db::value::CellValue::Integer(value)) => Some(*value as f64),
        Some(crate::db::value::CellValue::Float(value)) if value.is_finite() => Some(*value),
        _ => None,
    }
}

fn bool_value(value: Option<&crate::db::value::CellValue>) -> Option<bool> {
    match value {
        Some(crate::db::value::CellValue::Boolean(value)) => Some(*value),
        Some(crate::db::value::CellValue::Unsigned(value)) => Some(*value != 0),
        Some(crate::db::value::CellValue::Integer(value)) => Some(*value != 0),
        _ => None,
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn decode_error(field: &str) -> DatabaseError {
    DatabaseError {
        category: ErrorCategory::Configuration,
        code: Some("clickhouse_monitor_decode".into()),
        message: format!("ClickHouse monitor response is missing or has invalid {field}"),
        diagnostic: None,
    }
}

fn restricted_monitor_error(error: DatabaseError) -> DatabaseError {
    if error.category == ErrorCategory::Permission {
        DatabaseError {
            category: ErrorCategory::Permission,
            code: Some("clickhouse_monitor_restricted".into()),
            message: "ClickHouse monitoring visibility is restricted by server permissions".into(),
            diagnostic: None,
        }
    } else {
        error
    }
}
