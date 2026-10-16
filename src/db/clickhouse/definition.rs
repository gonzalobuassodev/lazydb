use sha2::{Digest, Sha256};

use crate::db::{
    DatabaseError, ErrorCategory,
    catalog::{CatalogKind, OptionalMetadata},
    catalog_mutation::{
        CatalogObjectDefinition, CatalogObjectDefinitionRequest, ClickHouseColumnDefinition,
        ClickHouseMaterializedViewDefinition, ClickHouseTableDefinition, ClickHouseViewDefinition,
    },
    value::CellValue,
};
use crate::security::sanitize_terminal_text;

use super::transport::ClickHouseAdapter;

impl ClickHouseAdapter {
    pub async fn load_catalog_object_definition(
        &self,
        request: &CatalogObjectDefinitionRequest,
    ) -> Result<CatalogObjectDefinition, DatabaseError> {
        if request.object.kind == CatalogKind::Dictionary {
            if request.connection.profile_id != self.profile_id()
                || request.object.native_path.len() != 3
                || request.object.native_path[0] != request.object.native_path[1]
                || request.target.profile_id != request.connection.profile_id
                || request.target.database != request.object.native_path[0]
                || !self.catalog_scope.allows_schema(
                    &request.object.native_path[0],
                    &request.object.native_path[0],
                )
            {
                return Err(definition_error(
                    "ClickHouse Dictionary definition request has a mismatched identity or scope",
                ));
            }
            return Err(DatabaseError::unsupported(
                "ClickHouse Dictionary definition editing is gated because SHOW CREATE may contain credentials; secret-bearing definitions are not loaded or replayed",
            ));
        }
        request
            .validate()
            .map_err(|error| definition_error(error.to_string()))?;
        if request.connection.profile_id != self.profile_id()
            || request.object.native_path.len() != 3
        {
            return Err(definition_error(
                "ClickHouse table definition request has a mismatched identity or database",
            ));
        }
        let database = request.object.native_path[0].as_str();
        let table = request.object.native_path[2].as_str();
        if request.object.native_path[1] != database
            || !self.catalog_scope.allows_schema(database, database)
        {
            return Err(definition_error(
                "ClickHouse table definition is outside the captured catalog scope",
            ));
        }
        if matches!(
            request.object.kind,
            CatalogKind::View | CatalogKind::MaterializedView
        ) {
            let ddl = self
                .relation_ddl_with_scope(&request.object, &self.catalog_scope)
                .await?;
            let canonical_ddl = sanitize_terminal_text(&ddl.sql);
            let fingerprint = format!("sha256:{:x}", Sha256::digest(canonical_ddl.as_bytes()));
            let (query, target) = extract_view_query_and_target(
                &canonical_ddl,
                request.object.kind,
            )
            .ok_or_else(|| {
                definition_error("ClickHouse view DDL has an unsupported AS/TO/ENGINE shape")
            })?;
            return if request.object.kind == CatalogKind::View {
                Ok(CatalogObjectDefinition::View(
                    crate::db::catalog_mutation::ViewDefinition {
                        database: database.into(),
                        schema: database.into(),
                        name: table.into(),
                        owner: String::new(),
                        comment: ddl.relation.comment.clone(),
                        query: query.clone(),
                        output_columns: Vec::new(),
                        security_barrier: crate::db::catalog_mutation::ViewOption::unavailable(
                            "ClickHouse does not expose this option",
                        ),
                        security_invoker: crate::db::catalog_mutation::ViewOption::unavailable(
                            "ClickHouse does not expose this option",
                        ),
                        check_option: crate::db::catalog_mutation::ViewOption::unavailable(
                            "ClickHouse does not expose this option",
                        ),
                        baseline_fingerprint: fingerprint.clone(),
                        clickhouse_native: Some(ClickHouseViewDefinition {
                            database: database.into(),
                            name: table.into(),
                            canonical_ddl,
                            query,
                            comment: ddl.relation.comment,
                            baseline_fingerprint: fingerprint.clone(),
                        }),
                    },
                ))
            } else {
                Ok(CatalogObjectDefinition::MaterializedView(
                    crate::db::catalog_mutation::MaterializedViewDefinition {
                        database: database.into(),
                        schema: database.into(),
                        name: table.into(),
                        owner: String::new(),
                        comment: ddl.relation.comment.clone(),
                        query: query.clone(),
                        tablespace: OptionalMetadata::Unsupported,
                        populated: false,
                        baseline_fingerprint: fingerprint.clone(),
                        clickhouse_native: Some(ClickHouseMaterializedViewDefinition {
                            database: database.into(),
                            name: table.into(),
                            canonical_ddl,
                            query: query.clone(),
                            target,
                            engine: ddl.relation.native_kind,
                            comment: ddl.relation.comment,
                            baseline_fingerprint: fingerprint,
                        }),
                    },
                ))
            };
        }
        if request.object.kind != CatalogKind::Table {
            return Err(definition_error(
                "ClickHouse native definition kind is unsupported",
            ));
        }
        let rows = self
            .catalog_rows(
                "SELECT name, type, position, default_kind, default_expression, comment FROM system.columns WHERE database = ? AND table = ? ORDER BY position, name",
                &[database, table],
            )
            .await?;
        if rows.is_empty() {
            return Err(definition_error("ClickHouse table no longer exists"));
        }
        let columns = rows
            .iter()
            .map(|row| {
                let name = text(row, 0)?;
                let native_type = text(row, 1)?;
                let position = unsigned(row, 2)?;
                let default_kind = text(row, 3)?.to_owned();
                let default_expression = text(row, 4)?.to_owned();
                let comment = text(row, 5)?.to_owned();
                Ok(ClickHouseColumnDefinition {
                    name: name.to_owned(),
                    ordinal_position: u32::try_from(position).map_err(|_| {
                        definition_error("ClickHouse column position exceeds supported range")
                    })?,
                    native_type: native_type.to_owned(),
                    default_kind,
                    default_expression,
                    comment: OptionalMetadata::Supported((!comment.is_empty()).then_some(comment)),
                })
            })
            .collect::<Result<Vec<_>, DatabaseError>>()?;
        let ddl = self
            .relation_ddl_with_scope(&request.object, &self.catalog_scope)
            .await?;
        let engine = ddl.relation.native_kind.clone();
        let comment = ddl.relation.comment.clone();
        let canonical_ddl = ddl.sql;
        let fingerprint = format!("sha256:{:x}", Sha256::digest(canonical_ddl.as_bytes()));
        Ok(CatalogObjectDefinition::ClickHouseTable(
            ClickHouseTableDefinition {
                database: database.to_owned(),
                name: table.to_owned(),
                engine,
                columns,
                canonical_ddl: sanitize_terminal_text(&canonical_ddl),
                baseline_fingerprint: fingerprint,
                comment,
            },
        ))
    }
}

pub fn extract_view_query_and_target(
    ddl: &str,
    kind: CatalogKind,
) -> Option<(String, Option<String>)> {
    let as_marker = find_top_level_as(ddl)?;
    let upper = ddl.to_ascii_uppercase();
    let after_as = &ddl[as_marker..];
    let upper_after_as = after_as.to_ascii_uppercase();
    let mut start = 0;
    if kind == CatalogKind::MaterializedView
        && let Some(populate) = upper_after_as.find("POPULATE")
    {
        start = populate + "POPULATE".len();
    }
    let query_tail = after_as[start..].trim_start();
    let upper_query_tail = query_tail.to_ascii_uppercase();
    let end = [" TO ", " ENGINE =", " POPULATE"]
        .iter()
        .filter_map(|marker| upper_query_tail.find(marker))
        .min()
        .unwrap_or(query_tail.len());
    let query = query_tail[..end].trim();
    if query.is_empty() {
        return None;
    }
    let target = if kind == CatalogKind::MaterializedView {
        let to = upper.find(" TO ").map(|at| &ddl[at + 4..]);
        to.map(|tail| {
            let tail = tail.trim_start();
            let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
            tail[..end].replace("``", "").replace('`', "")
        })
    } else {
        None
    };
    Some((query.to_owned(), target))
}

fn find_top_level_as(sql: &str) -> Option<usize> {
    let bytes = sql.as_bytes();
    let mut quote = None;
    let mut depth = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(delimiter) = quote {
            if byte == delimiter {
                if bytes.get(index + 1) == Some(&delimiter) {
                    index += 1;
                } else {
                    quote = None;
                }
            } else if byte == b'\\' {
                index += 1;
            }
        } else {
            match byte {
                b'\'' | b'"' | b'`' => quote = Some(byte),
                b'(' => depth += 1,
                b')' => depth = depth.saturating_sub(1),
                b'A' | b'a' if depth == 0 && index + 1 < bytes.len() => {
                    if (bytes[index + 1] == b'S' || bytes[index + 1] == b's')
                        && (index == 0 || bytes[index - 1].is_ascii_whitespace())
                        && bytes.get(index + 2).is_some_and(u8::is_ascii_whitespace)
                    {
                        return Some(index + 3);
                    }
                }
                _ => {}
            }
        }
        index += 1;
    }
    None
}

fn text(row: &[CellValue], index: usize) -> Result<&str, DatabaseError> {
    match row.get(index) {
        Some(CellValue::Text(value)) => Ok(value),
        _ => Err(definition_error(
            "ClickHouse definition returned invalid text metadata",
        )),
    }
}

fn unsigned(row: &[CellValue], index: usize) -> Result<u64, DatabaseError> {
    match row.get(index) {
        Some(CellValue::Unsigned(value)) => Ok(*value),
        Some(CellValue::Integer(value)) if *value >= 0 => Ok(*value as u64),
        _ => Err(definition_error(
            "ClickHouse definition returned invalid ordinal metadata",
        )),
    }
}

fn definition_error(message: impl Into<String>) -> DatabaseError {
    DatabaseError {
        category: ErrorCategory::Configuration,
        code: Some("invalid_clickhouse_definition_request".into()),
        message: sanitize_terminal_text(&message.into()),
        diagnostic: None,
    }
}
