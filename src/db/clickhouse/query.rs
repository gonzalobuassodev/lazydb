use crate::db::{
    DatabaseError, ErrorCategory,
    query::{ColumnMeta, QueryBudget, QueryOutcome, QueryOutcomeAccumulator},
};
use crate::security::sanitize_terminal_text;
use futures_util::StreamExt;

use super::{
    transport::ClickHouseAdapter,
    value::{WireColumn, parse_names_types_header, parse_tsv_row},
};

const OUTPUT_FORMAT: &str = "TabSeparatedWithNamesAndTypes";
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

impl ClickHouseAdapter {
    pub async fn execute(&self, sql: &str) -> Result<QueryOutcome, DatabaseError> {
        self.execute_with_budget(sql, QueryBudget::UNBOUNDED).await
    }

    pub(crate) async fn execute_with_budget(
        &self,
        sql: &str,
        budget: QueryBudget,
    ) -> Result<QueryOutcome, DatabaseError> {
        self.execute_statement_with_budget(sql, budget).await
    }

    pub(crate) async fn execute_statement(&self, sql: &str) -> Result<QueryOutcome, DatabaseError> {
        self.execute_statement_with_budget(sql, QueryBudget::UNBOUNDED)
            .await
    }

    async fn execute_statement_with_budget(
        &self,
        sql: &str,
        budget: QueryBudget,
    ) -> Result<QueryOutcome, DatabaseError> {
        let statement = if is_row_returning_query(sql) {
            append_grid_format(sql)?
        } else {
            sql.trim().trim_end_matches(';').trim_end().to_owned()
        };
        if statement.is_empty() {
            return Err(DatabaseError::configuration("SQL query is empty"));
        }
        let response = self.query_response(&statement).await?;
        let mut stream = response.bytes_stream();
        let mut accumulator = QueryOutcomeAccumulator::with_budget(budget);
        let mut buffer = Vec::new();
        let mut names: Option<String> = None;
        let mut columns: Option<Vec<WireColumn>> = None;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| DatabaseError {
                category: ErrorCategory::Network,
                code: None,
                message: sanitize_terminal_text(&error.to_string()),
                diagnostic: None,
            })?;
            buffer.extend_from_slice(&chunk);
            if buffer.len() > MAX_LINE_BYTES && !buffer.contains(&b'\n') {
                return Err(invalid_response(
                    "ClickHouse TSV row exceeds the 8 MiB safety limit",
                ));
            }
            while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
                let line = buffer.drain(..=newline).collect::<Vec<_>>();
                let line = &line[..line.len() - 1];
                let line = std::str::from_utf8(line)
                    .map_err(|_| invalid_response("ClickHouse metadata and rows must be UTF-8"))?;
                if columns.is_none() && names.is_none() && line.is_empty() {
                    continue;
                }
                if columns.is_none() && names.is_none() {
                    names = Some(line.to_owned());
                    continue;
                }
                if columns.is_none() {
                    columns = Some(
                        parse_names_types_header(names.take().unwrap_or_default().as_str(), line)
                            .map_err(|message| invalid_response(&message))?,
                    );
                    accumulator.metadata(to_column_meta(columns.as_deref().unwrap_or_default()));
                    if budget.max_rows == 0 || budget.max_bytes == 0 {
                        drain_bounded(&mut stream).await?;
                        accumulator.mark_truncated();
                        return Ok(accumulator.finish_truncated());
                    }
                    continue;
                }
                let row = parse_tsv_row(line, columns.as_deref().unwrap_or_default())
                    .map_err(|message| invalid_response(&message))?;
                accumulator.row(to_column_meta(columns.as_deref().unwrap_or_default()), row);
                if accumulator.budget_exhausted() {
                    drain_bounded(&mut stream).await?;
                    return Ok(accumulator.finish_truncated());
                }
            }
            if buffer.len() > MAX_LINE_BYTES {
                return Err(invalid_response(
                    "ClickHouse TSV row exceeds the 8 MiB safety limit",
                ));
            }
        }
        if !buffer.is_empty() {
            return Err(invalid_response("ClickHouse TSV response ended mid-row"));
        }
        Ok(accumulator.finish())
    }
}

async fn drain_bounded<S, B, E>(stream: &mut S) -> Result<(), DatabaseError>
where
    S: futures_util::Stream<Item = Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut pending_line_bytes = 0usize;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| DatabaseError {
            category: ErrorCategory::Network,
            code: None,
            message: sanitize_terminal_text(&error.to_string()),
            diagnostic: None,
        })?;
        let mut start = 0;
        for (index, byte) in chunk.as_ref().iter().enumerate() {
            if *byte != b'\n' {
                continue;
            }
            pending_line_bytes = pending_line_bytes.saturating_add(index.saturating_sub(start));
            if pending_line_bytes > MAX_LINE_BYTES {
                return Err(invalid_response(
                    "ClickHouse TSV row exceeds the 8 MiB safety limit",
                ));
            }
            pending_line_bytes = 0;
            start = index + 1;
        }
        if start < chunk.as_ref().len() {
            pending_line_bytes = pending_line_bytes.saturating_add(chunk.as_ref().len() - start);
            if pending_line_bytes > MAX_LINE_BYTES {
                return Err(invalid_response(
                    "ClickHouse TSV row exceeds the 8 MiB safety limit",
                ));
            }
        }
    }
    Ok(())
}

fn to_column_meta(columns: &[super::value::WireColumn]) -> Vec<ColumnMeta> {
    columns
        .iter()
        .map(|column| ColumnMeta {
            name: column.name.clone(),
            type_name: column.type_name.clone(),
        })
        .collect()
}

fn invalid_response(message: &str) -> DatabaseError {
    DatabaseError {
        category: ErrorCategory::Sql,
        code: Some("invalid_clickhouse_response".to_owned()),
        message: sanitize_terminal_text(message),
        diagnostic: None,
    }
}

fn append_grid_format(sql: &str) -> Result<String, DatabaseError> {
    let trimmed = sql.trim_end();
    if trimmed.is_empty() {
        return Err(DatabaseError::configuration("SQL query is empty"));
    }
    let without_semicolon = trimmed.strip_suffix(';').unwrap_or(trimmed).trim_end();
    if without_semicolon.contains(';') {
        return Err(DatabaseError::unsupported(
            "ClickHouse grid execution accepts one statement at a time",
        ));
    }
    if has_top_level_format(without_semicolon) {
        return Err(DatabaseError::unsupported(
            "ClickHouse query FORMAT clauses are not yet supported by the grid adapter",
        ));
    }
    Ok(format!("{without_semicolon} FORMAT {OUTPUT_FORMAT}"))
}

fn is_row_returning_query(sql: &str) -> bool {
    let first = sql
        .trim_start()
        .split(|character: char| !character.is_ascii_alphabetic())
        .next()
        .unwrap_or_default();
    matches!(
        first.to_ascii_uppercase().as_str(),
        "SELECT" | "WITH" | "EXPLAIN" | "SHOW" | "DESCRIBE" | "DESC"
    )
}

fn has_top_level_format(sql: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    let bytes = sql.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let character = bytes[index] as char;
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == delimiter {
                if bytes.get(index + 1) == Some(&(delimiter as u8)) {
                    index += 1;
                } else {
                    quote = None;
                }
            }
        } else if character == '-' && bytes.get(index + 1) == Some(&b'-') {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        } else if character == '#' {
            index += 1;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        } else if character == '/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
            continue;
        } else if matches!(character, '\'' | '"' | '`') {
            quote = Some(character);
        } else if character.eq_ignore_ascii_case(&'F')
            && sql[index..]
                .get(..6)
                .is_some_and(|part| part.eq_ignore_ascii_case("format"))
        {
            let before =
                index == 0 || !bytes[index - 1].is_ascii_alphanumeric() && bytes[index - 1] != b'_';
            let after = bytes
                .get(index + 6)
                .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_');
            if before && after {
                return true;
            }
        }
        index += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::append_grid_format;

    #[test]
    fn query_format_is_added_without_rewriting_sql_body() {
        assert_eq!(
            append_grid_format("SELECT 1 AS n;\n").unwrap(),
            "SELECT 1 AS n FORMAT TabSeparatedWithNamesAndTypes"
        );
    }

    #[test]
    fn explicit_output_formats_are_rejected_until_normalization_is_safe() {
        assert!(append_grid_format("SELECT 1 FORMAT JSON").is_err());
        assert!(append_grid_format("SELECT ' FORMAT JSON' AS n").is_ok());
        assert!(append_grid_format("SELECT 1 AS n").is_ok());
        assert!(append_grid_format("SELECT 1; SELECT 2").is_err());
    }

    #[test]
    fn top_level_format_detection_ignores_literals_and_comments() {
        assert!(!super::has_top_level_format(
            "SELECT 'format JSON', /* FORMAT XML */ 1"
        ));
        assert!(super::has_top_level_format("SELECT 1 FORMAT JSON"));
    }
}
