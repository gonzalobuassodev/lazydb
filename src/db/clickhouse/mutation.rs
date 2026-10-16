use crate::db::{
    catalog::{CatalogEntry, CatalogId, CatalogKind, CatalogTarget},
    catalog_drop::{CatalogDropError, CatalogDropPlan, CatalogDropRequest},
    catalog_mutation::{
        CatalogMutationAnchor, CatalogMutationAvailability, CatalogMutationCapabilities,
        CatalogMutationError, CatalogMutationExecutionMode, CatalogMutationMode,
        CatalogMutationPlan, CatalogMutationTarget, CatalogObjectType, CatalogSelectionHint,
    },
};

use super::ClickHouseAdapter;

impl ClickHouseAdapter {
    pub fn catalog_mutation_capabilities() -> CatalogMutationCapabilities {
        use crate::db::catalog_mutation::CatalogMutationOption;
        let database = CatalogObjectType::Catalog(CatalogKind::Database);
        let table = CatalogObjectType::Catalog(CatalogKind::Table);
        let view = CatalogObjectType::Catalog(CatalogKind::View);
        let skipping_index = CatalogObjectType::Catalog(CatalogKind::Index);
        let check = CatalogObjectType::Catalog(CatalogKind::CheckConstraint);
        CatalogMutationCapabilities {
            profile_create: vec![CatalogMutationOption {
                object_type: database,
                availability: CatalogMutationAvailability::Available,
            }],
            create: vec![
                CatalogMutationOption {
                    object_type: table,
                    availability: CatalogMutationAvailability::Available,
                },
                CatalogMutationOption {
                    object_type: view,
                    availability: CatalogMutationAvailability::Available,
                },
                CatalogMutationOption {
                    object_type: CatalogObjectType::Catalog(CatalogKind::MaterializedView),
                    availability: CatalogMutationAvailability::Unavailable {
                        reason: "ClickHouse materialized view creation requires explicit target and engine controls",
                    },
                },
                CatalogMutationOption {
                    object_type: skipping_index,
                    availability: CatalogMutationAvailability::Unavailable {
                        reason: "ClickHouse skipping indexes require a native table baseline and explicit TYPE/GRANULARITY controls",
                    },
                },
                CatalogMutationOption {
                    object_type: check,
                    availability: CatalogMutationAvailability::Unavailable {
                        reason: "ClickHouse check constraints require a native table baseline and server-version-aware ALTER controls",
                    },
                },
            ],
            edit: vec![
                CatalogMutationOption {
                    object_type: table,
                    availability: CatalogMutationAvailability::Available,
                },
                CatalogMutationOption {
                    object_type: view,
                    availability: CatalogMutationAvailability::Available,
                },
                CatalogMutationOption {
                    object_type: CatalogObjectType::Catalog(CatalogKind::MaterializedView),
                    availability: CatalogMutationAvailability::Unavailable {
                        reason: "ClickHouse materialized view edits require native target and engine controls",
                    },
                },
                CatalogMutationOption {
                    object_type: skipping_index,
                    availability: CatalogMutationAvailability::Unavailable {
                        reason: "ClickHouse skipping-index definition loading and ALTER planning are not implemented",
                    },
                },
                CatalogMutationOption {
                    object_type: check,
                    availability: CatalogMutationAvailability::Unavailable {
                        reason: "ClickHouse check-constraint definition loading and ALTER planning are not implemented",
                    },
                },
            ],
            ..CatalogMutationCapabilities::default()
        }
    }

    pub fn plan_catalog_drop(
        request: CatalogDropRequest,
        entry: &CatalogEntry,
    ) -> Result<CatalogDropPlan, CatalogDropError> {
        request.validate()?;
        if request.object != entry.id
            || request.connection.profile_id != entry.id.profile_id()
            || entry.id.kind != entry.kind
        {
            return Err(CatalogDropError::ObjectMismatch);
        }
        let sql = match entry.kind {
            CatalogKind::Database => {
                if entry.id.native_path.len() != 1
                    || entry.qualified_name.object != entry.id.native_path[0]
                {
                    return Err(CatalogDropError::InvalidMetadata {
                        kind: entry.kind,
                        reason: "database identity must have one path segment".into(),
                    });
                }
                let database = &entry.id.native_path[0];
                let _maintenance = request
                    .maintenance_database
                    .as_deref()
                    .filter(|value| !value.trim().is_empty() && *value != database)
                    .ok_or_else(|| CatalogDropError::Unsupported {
                        kind: entry.kind,
                        reason: "database drop requires a distinct maintenance database".into(),
                    })?;
                format!("DROP DATABASE {}", quote_identifier(database))
            }
            CatalogKind::Table | CatalogKind::View | CatalogKind::MaterializedView => {
                let (database, name) = relation_names(&entry.id)?;
                if entry.qualified_name.object != name
                    || entry.id.native_path[0] != entry.id.native_path[1]
                {
                    return Err(CatalogDropError::InvalidMetadata {
                        kind: entry.kind,
                        reason: "ClickHouse relation identity and qualified name disagree".into(),
                    });
                }
                let noun = match entry.kind {
                    CatalogKind::View => "VIEW",
                    _ => "TABLE",
                };
                format!(
                    "DROP {noun} {}.{}",
                    quote_identifier(database),
                    quote_identifier(name)
                )
            }
            kind => {
                let reason = match kind {
                    CatalogKind::Projection => {
                        "ClickHouse Projection drop requires a parent-table ALTER definition and is not yet implemented"
                    }
                    CatalogKind::Dictionary => {
                        "ClickHouse Dictionary drop is gated until credential-safe native identity and confirmation are available"
                    }
                    CatalogKind::Index => {
                        "ClickHouse skipping-index drop requires native table ownership and is not yet implemented"
                    }
                    CatalogKind::CheckConstraint => {
                        "ClickHouse check-constraint drop requires native table ownership and is not yet implemented"
                    }
                    _ => "ClickHouse drop is not implemented for this object kind",
                };
                return Err(CatalogDropError::Unsupported {
                    kind,
                    reason: reason.into(),
                });
            }
        };
        let mut plan = CatalogDropPlan::new(request.clone(), entry, sql)?;
        if entry.kind == CatalogKind::Database {
            plan = plan.with_execution_target(
                crate::db::catalog_drop::CatalogDropExecutionTarget::MaintenanceDatabase(
                    request
                        .maintenance_database
                        .clone()
                        .expect("validated maintenance target"),
                ),
            );
        }
        if entry.kind == CatalogKind::Database
            && request
                .maintenance_database
                .as_deref()
                .is_some_and(|maintenance| {
                    maintenance
                        == entry
                            .id
                            .native_path
                            .first()
                            .map(String::as_str)
                            .unwrap_or_default()
                })
        {
            return Err(CatalogDropError::Unsupported {
                kind: entry.kind,
                reason: "cannot drop the database used for the maintenance connection".into(),
            });
        }
        plan.validate()?;
        Ok(plan)
    }

    pub fn plan_catalog_mutation(
        request: crate::db::catalog_mutation::CatalogMutationRequest,
        draft: crate::model::catalog_editor::CatalogDraft,
        baseline: Option<crate::db::catalog_mutation::CatalogObjectDefinition>,
        current_database: &str,
    ) -> Result<CatalogMutationPlan, CatalogMutationError> {
        use crate::model::catalog_editor::CatalogDraft;
        if request.mode == CatalogMutationMode::Edit {
            return match request.object_type {
                CatalogObjectType::Catalog(CatalogKind::Table) => {
                    plan_clickhouse_table_edit(request, draft, baseline, current_database)
                }
                CatalogObjectType::Catalog(CatalogKind::View) => {
                    plan_clickhouse_view_edit(request, draft, baseline, current_database)
                }
                CatalogObjectType::Catalog(CatalogKind::MaterializedView) => {
                    plan_clickhouse_materialized_view_edit(
                        request,
                        draft,
                        baseline,
                        current_database,
                    )
                }
                _ => Err(CatalogMutationError::UnsupportedOperation {
                    object_type: request.object_type,
                }),
            };
        }
        if baseline.is_some() {
            return Err(CatalogMutationError::UnsupportedOperation {
                object_type: request.object_type,
            });
        }
        let (name, sql, target, object_kind) = match (request.object_type, draft) {
            (
                CatalogObjectType::Catalog(CatalogKind::Database),
                CatalogDraft::Database(database),
            ) => {
                database.validate()?;
                let name = database.name.value().trim().to_owned();
                let CatalogMutationAnchor::Profile { profile_id } = &request.anchor else {
                    return Err(CatalogMutationError::InvalidAnchor {
                        reason: "ClickHouse database creation requires a profile anchor",
                    });
                };
                if *profile_id != request.connection.profile_id {
                    return Err(CatalogMutationError::ProfileMismatch {
                        object_profile_id: *profile_id,
                        connection_profile_id: request.connection.profile_id,
                    });
                }
                if database.database_kind != crate::profile::DatabaseKind::ClickHouse {
                    return Err(CatalogMutationError::InvalidDraft {
                        reason: "ClickHouse database draft required".into(),
                    });
                }
                let target = if current_database.trim().is_empty() {
                    return Err(CatalogMutationError::InvalidAnchor {
                        reason: "ClickHouse maintenance database is empty",
                    });
                } else {
                    current_database.to_owned()
                };
                (
                    name.clone(),
                    format!("CREATE DATABASE {}", quote_identifier(&name)),
                    CatalogMutationTarget::maintenance(target)?,
                    CatalogKind::Database,
                )
            }
            (CatalogObjectType::Catalog(CatalogKind::Table), CatalogDraft::Table(table)) => {
                table.validate()?;
                if table.database_kind != crate::profile::DatabaseKind::ClickHouse {
                    return Err(CatalogMutationError::InvalidDraft {
                        reason: "ClickHouse table draft required".into(),
                    });
                }
                let engine = table.clickhouse_engine.value().trim();
                let (engine_name, requires_order) = if engine.eq_ignore_ascii_case("mergetree") {
                    ("MergeTree", true)
                } else if engine.eq_ignore_ascii_case("memory") {
                    ("Memory", false)
                } else {
                    return Err(CatalogMutationError::InvalidDraft {
                        reason: "supported engines are MergeTree and Memory".into(),
                    });
                };
                let mut columns = Vec::new();
                for column in &table.columns {
                    if matches!(
                        column.state,
                        crate::model::catalog_editor::DraftRowState::Removed { .. }
                    ) {
                        continue;
                    }
                    if column.identity
                        || !column.generated_expression.value().trim().is_empty()
                        || !column.collation.value().trim().is_empty()
                        || !column.comment.value().trim().is_empty()
                    {
                        return Err(CatalogMutationError::InvalidDraft { reason: "ClickHouse create does not support identity, generated, collation or inline comment column modifiers".into() });
                    }
                    let name = column.name.value().trim();
                    let data_type = column.native_type.value().trim();
                    if name.is_empty()
                        || data_type.is_empty()
                        || column.nullable
                        || !column.default_expression.value().trim().is_empty()
                    {
                        return Err(CatalogMutationError::InvalidDraft { reason: "ClickHouse create currently supports named columns without nullable/default modifiers".into() });
                    }
                    columns.push(format!("{} {}", quote_identifier(name), data_type));
                }
                if columns.is_empty() {
                    return Err(CatalogMutationError::InvalidDraft {
                        reason: "at least one ClickHouse column is required".into(),
                    });
                }
                if !table.comment.value().trim().is_empty()
                    || !table.owner.value().trim().is_empty()
                {
                    return Err(CatalogMutationError::InvalidDraft { reason: "ClickHouse table owner/comment clauses are not supported by this create operation".into() });
                }
                let mut clauses = format!(" ENGINE = {engine_name}");
                let order = table.clickhouse_order_by.value().trim();
                if requires_order {
                    clauses.push_str(&format!(
                        " ORDER BY {}",
                        if order.is_empty() { "tuple()" } else { order }
                    ));
                }
                let partition = table.clickhouse_partition_by.value().trim();
                let ttl = table.clickhouse_ttl.value().trim();
                if !partition.is_empty() {
                    clauses.push_str(&format!(" PARTITION BY {partition}"));
                }
                if !ttl.is_empty() {
                    clauses.push_str(&format!(" TTL {ttl}"));
                }
                let name = table.name.value().trim().to_owned();
                let database = table.schema.value().trim();
                let (anchor_schema, group) = match &request.anchor {
                    CatalogMutationAnchor::Catalog(id) => (id, None),
                    CatalogMutationAnchor::Group { schema, group } => (schema, Some(*group)),
                    _ => {
                        return Err(CatalogMutationError::InvalidAnchor {
                            reason: "ClickHouse table creation requires a table group anchor",
                        });
                    }
                };
                if anchor_schema.profile_id() != request.connection.profile_id
                    || anchor_schema.native_path.len() != 2
                    || anchor_schema.native_path[0] != database
                    || anchor_schema.native_path[1] != database
                    || group.is_some_and(|group| group != crate::db::catalog::ObjectGroup::Tables)
                {
                    return Err(CatalogMutationError::InvalidAnchor {
                        reason: "ClickHouse table draft does not match the selected database group",
                    });
                }
                (
                    name.clone(),
                    format!(
                        "CREATE TABLE {}.{} ({}){clauses}",
                        quote_identifier(database),
                        quote_identifier(&name),
                        columns.join(", ")
                    ),
                    CatalogMutationTarget::maintenance(database.to_owned())?,
                    CatalogKind::Table,
                )
            }
            (CatalogObjectType::Catalog(CatalogKind::View), CatalogDraft::View(view)) => {
                view.validate()?;
                let (object, group) = match &request.anchor {
                    CatalogMutationAnchor::Catalog(object) => (object, None),
                    CatalogMutationAnchor::Group { schema, group } => (schema, Some(*group)),
                    _ => {
                        return Err(CatalogMutationError::InvalidAnchor {
                            reason: "ClickHouse view creation requires a database view-group anchor",
                        });
                    }
                };
                if object.kind != CatalogKind::Schema
                    || object.native_path.len() != 2
                    || object.native_path[0] != object.native_path[1]
                    || view.schema.value() != object.native_path[0]
                    || !view.owner.value().is_empty()
                    || !view.output_columns.value().is_empty()
                    || view.security_barrier.value.is_some()
                    || view.security_invoker.value.is_some()
                    || view.check_option.value.is_some()
                    || !view.comment.value().is_empty()
                    || group.is_some_and(|group| group != crate::db::catalog::ObjectGroup::Views)
                {
                    return Err(CatalogMutationError::InvalidDraft { reason: "ClickHouse view create supports only database, name and SELECT query fields".into() });
                }
                let database = object.native_path[0].as_str();
                if database.is_empty() || !safe_view_query(view.query.value()) {
                    return Err(CatalogMutationError::InvalidDraft {
                        reason: "ClickHouse view query or captured database is invalid".into(),
                    });
                }
                let sql = format!(
                    "CREATE VIEW {}.{} AS {}",
                    quote_identifier(database),
                    quote_identifier(view.name.value().trim()),
                    view.query.value().trim()
                );
                (
                    view.name.value().trim().into(),
                    sql,
                    CatalogMutationTarget::maintenance(database)?,
                    CatalogKind::View,
                )
            }
            (_, _) => {
                return Err(CatalogMutationError::UnsupportedOperation {
                    object_type: request.object_type,
                });
            }
        };
        if request.current_database.as_deref() != Some(current_database) {
            return Err(CatalogMutationError::StaleState);
        }
        let (refresh, selection) = match &request.anchor {
            CatalogMutationAnchor::Profile { profile_id }
                if object_kind == CatalogKind::Database
                    && *profile_id == request.connection.profile_id =>
            {
                let id = CatalogId::new(*profile_id, object_kind, [&name]);
                (
                    vec![CatalogTarget::Databases],
                    CatalogSelectionHint::Object(id),
                )
            }
            CatalogMutationAnchor::Group { schema, group }
                if (object_kind == CatalogKind::Table
                    && *group == crate::db::catalog::ObjectGroup::Tables)
                    || (object_kind == CatalogKind::View
                        && *group == crate::db::catalog::ObjectGroup::Views) =>
            {
                let id = CatalogId::new(
                    request.connection.profile_id,
                    object_kind,
                    schema
                        .native_path
                        .iter()
                        .map(String::as_str)
                        .chain(std::iter::once(name.as_str())),
                );
                let refresh = CatalogTarget::objects(schema.clone(), *group).map_err(|error| {
                    CatalogMutationError::InvalidPlan {
                        reason: error.to_string(),
                    }
                })?;
                (vec![refresh], CatalogSelectionHint::Object(id))
            }
            CatalogMutationAnchor::Catalog(schema)
                if (object_kind == CatalogKind::Table || object_kind == CatalogKind::View)
                    && schema.kind == CatalogKind::Schema
                    && schema.profile_id() == request.connection.profile_id =>
            {
                let id = CatalogId::new(
                    request.connection.profile_id,
                    object_kind,
                    schema
                        .native_path
                        .iter()
                        .map(String::as_str)
                        .chain(std::iter::once(name.as_str())),
                );
                let group = if object_kind == CatalogKind::View {
                    crate::db::catalog::ObjectGroup::Views
                } else {
                    crate::db::catalog::ObjectGroup::Tables
                };
                let refresh = CatalogTarget::objects(schema.clone(), group).map_err(|error| {
                    CatalogMutationError::InvalidPlan {
                        reason: error.to_string(),
                    }
                })?;
                (vec![refresh], CatalogSelectionHint::Object(id))
            }
            _ => {
                return Err(CatalogMutationError::InvalidAnchor {
                    reason: "ClickHouse create requires a profile database anchor or table group anchor",
                });
            }
        };
        CatalogMutationPlan::new(
            request.clone(),
            request.object_type,
            CatalogMutationExecutionMode::Autocommit,
            target,
            refresh,
            selection,
            None,
            Vec::new(),
            vec![sql],
        )
    }

    pub async fn execute_catalog_mutation(
        &self,
        plan: &CatalogMutationPlan,
    ) -> Result<crate::db::query::QueryOutcome, crate::db::DatabaseError> {
        plan.validate()
            .map_err(|error| crate::db::DatabaseError::configuration(error.to_string()))?;
        if plan.execution_mode != CatalogMutationExecutionMode::Autocommit || plan.step_count() != 1
        {
            return Err(crate::db::DatabaseError::configuration(
                "ClickHouse catalog mutation requires one autocommit statement on its captured maintenance database",
            ));
        }
        self.execute_statement(&plan.statements()[0]).await
    }
}

fn plan_clickhouse_table_edit(
    request: crate::db::catalog_mutation::CatalogMutationRequest,
    draft: crate::model::catalog_editor::CatalogDraft,
    baseline: Option<crate::db::catalog_mutation::CatalogObjectDefinition>,
    current_database: &str,
) -> Result<CatalogMutationPlan, CatalogMutationError> {
    use crate::db::catalog_mutation::CatalogObjectDefinition;
    use crate::model::catalog_editor::{CatalogDraft, DraftRowState};
    if request.object_type != CatalogObjectType::Catalog(CatalogKind::Table) {
        return Err(CatalogMutationError::UnsupportedOperation {
            object_type: request.object_type,
        });
    }
    let Some(CatalogObjectDefinition::ClickHouseTable(baseline)) = baseline else {
        return Err(CatalogMutationError::InvalidDraft {
            reason: "ClickHouse edit requires a native table baseline".into(),
        });
    };
    let CatalogDraft::Table(table) = draft else {
        return Err(CatalogMutationError::InvalidDraft {
            reason: "ClickHouse table draft required".into(),
        });
    };
    table.validate()?;
    let CatalogMutationAnchor::Catalog(object) = &request.anchor else {
        return Err(CatalogMutationError::InvalidAnchor {
            reason: "ClickHouse table edit requires the exact table anchor",
        });
    };
    if object.kind != CatalogKind::Table
        || object.profile_id() != request.connection.profile_id
        || object.native_path.len() != 3
        || object.native_path[0] != baseline.database
        || object.native_path[2] != baseline.name
        || table.schema.value() != baseline.database
        || request.current_database.as_deref() != Some(current_database)
    {
        return Err(CatalogMutationError::StaleState);
    }
    if table.clickhouse_baseline_fingerprint.as_deref()
        != Some(baseline.baseline_fingerprint.as_str())
        || table.clickhouse_native_ddl.as_deref() != Some(baseline.canonical_ddl.as_str())
    {
        return Err(CatalogMutationError::StaleState);
    }
    if table.owner.value() != ""
        || !table.indexes.is_empty()
        || !table.constraints.is_empty()
        || table.clickhouse_engine.value() != baseline.engine
        || (!table.clickhouse_order_by.value().is_empty()
            && clickhouse_order_by_clause(&baseline.canonical_ddl).as_deref()
                != Some(table.clickhouse_order_by.value()))
        || (!table.clickhouse_partition_by.value().is_empty()
            && !baseline
                .canonical_ddl
                .contains(table.clickhouse_partition_by.value()))
        || (!table.clickhouse_ttl.value().is_empty()
            && !baseline
                .canonical_ddl
                .contains(table.clickhouse_ttl.value()))
        || baseline.columns.len()
            != table
                .columns
                .iter()
                .filter(|column| !matches!(column.state, DraftRowState::Added))
                .count()
        || table.clickhouse_baseline_columns.as_deref() != Some(baseline.columns.as_slice())
    {
        return Err(CatalogMutationError::InvalidDraft { reason: "ClickHouse edit only accepts rename, table comment, or one added column; engine/key/TTL/options edits are gated".into() });
    }
    let draft_existing_order = table
        .columns
        .iter()
        .filter(|column| matches!(column.state, DraftRowState::Existing { .. }))
        .filter_map(|column| column.existing_name.as_deref())
        .collect::<Vec<_>>();
    let native_order = baseline
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<Vec<_>>();
    if draft_existing_order != native_order {
        return Err(CatalogMutationError::InvalidDraft {
            reason: "ClickHouse column reordering is not supported by this edit plan".into(),
        });
    }
    let changed_name = table.name.value() != baseline.name;
    let baseline_comment = table
        .clickhouse_baseline_comment
        .as_deref()
        .unwrap_or_default();
    let changed_comment = table.comment.value() != baseline_comment;
    let added = table
        .columns
        .iter()
        .filter(|column| matches!(column.state, DraftRowState::Added))
        .collect::<Vec<_>>();
    let changed_existing = table
        .columns
        .iter()
        .filter(|column| !matches!(column.state, DraftRowState::Added))
        .any(|column| match &column.state {
            DraftRowState::Removed { .. } => true,
            DraftRowState::Existing { .. } => baseline
                .columns
                .iter()
                .find(|base| Some(base.name.as_str()) == column.existing_name.as_deref())
                .is_none_or(|base| {
                    base.native_type != column.native_type.value()
                        || base.default_expression != column.default_expression.value()
                        || column.generated_expression.value()
                            != if matches!(base.default_kind.as_str(), "MATERIALIZED" | "ALIAS") {
                                base.default_expression.as_str()
                            } else {
                                ""
                            }
                        || column.nullable != base.native_type.starts_with("Nullable(")
                        || column.identity
                        || !column.collation.value().is_empty()
                        || match &base.comment {
                            crate::db::catalog::OptionalMetadata::Unsupported => true,
                            crate::db::catalog::OptionalMetadata::Supported(value) => {
                                value.as_deref().unwrap_or_default() != column.comment.value()
                            }
                        }
                }),
            DraftRowState::Added => false,
        });
    if changed_existing
        || added.len() > 1
        || (added.len() == 1 && (changed_name || changed_comment))
        || usize::from(changed_name) + usize::from(changed_comment) + usize::from(added.len() == 1)
            == 0
    {
        return Err(CatalogMutationError::InvalidDraft { reason: "ClickHouse edit shape is unsupported; apply one rename, comment change, or simple column addition per operation".into() });
    }
    let (sql, new_name) = if changed_name {
        (
            format!(
                "RENAME TABLE {}.{} TO {}.{}",
                quote_identifier(&baseline.database),
                quote_identifier(&baseline.name),
                quote_identifier(&baseline.database),
                quote_identifier(table.name.value().trim())
            ),
            table.name.value().trim().to_owned(),
        )
    } else if changed_comment {
        let escaped = table
            .comment
            .value()
            .replace('\\', "\\\\")
            .replace('\'', "\\'");
        (
            format!(
                "ALTER TABLE {}.{} MODIFY COMMENT '{}'",
                quote_identifier(&baseline.database),
                quote_identifier(&baseline.name),
                escaped
            ),
            baseline.name.clone(),
        )
    } else {
        let column = added[0];
        if !matches!(
            table.columns.last().map(|column| &column.state),
            Some(DraftRowState::Added)
        ) {
            return Err(CatalogMutationError::InvalidDraft {
                reason: "ClickHouse added column must be appended after existing columns".into(),
            });
        }
        if column.name.value().trim().is_empty()
            || column.native_type.value().trim().is_empty()
            || column.nullable
            || !column.default_expression.value().is_empty()
            || !column.generated_expression.value().is_empty()
            || !column.comment.value().is_empty()
            || column.identity
            || !column.collation.value().is_empty()
        {
            return Err(CatalogMutationError::InvalidDraft {
                reason: "ClickHouse ADD COLUMN currently supports a name and native type only"
                    .into(),
            });
        }
        let existing_name = baseline
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<std::collections::HashSet<_>>();
        if existing_name.contains(column.name.value().trim())
            || !safe_clickhouse_type(column.native_type.value().trim())
        {
            return Err(CatalogMutationError::InvalidDraft {
                reason: "ClickHouse ADD COLUMN name or type is invalid/unsupported".into(),
            });
        }
        (
            format!(
                "ALTER TABLE {}.{} ADD COLUMN {} {}",
                quote_identifier(&baseline.database),
                quote_identifier(&baseline.name),
                quote_identifier(column.name.value().trim()),
                column.native_type.value().trim()
            ),
            baseline.name.clone(),
        )
    };
    let schema = CatalogId::new(
        request.connection.profile_id,
        CatalogKind::Schema,
        [baseline.database.clone(), baseline.database.clone()],
    );
    let refresh = CatalogTarget::objects(schema, crate::db::catalog::ObjectGroup::Tables).map_err(
        |error| CatalogMutationError::InvalidPlan {
            reason: error.to_string(),
        },
    )?;
    let selection = CatalogSelectionHint::Object(CatalogId::new(
        request.connection.profile_id,
        CatalogKind::Table,
        [
            baseline.database.as_str(),
            baseline.database.as_str(),
            new_name.as_str(),
        ],
    ));
    CatalogMutationPlan::new(
        request.clone(),
        request.object_type,
        CatalogMutationExecutionMode::Autocommit,
        CatalogMutationTarget::maintenance(baseline.database)?,
        vec![refresh],
        selection,
        Some(baseline.baseline_fingerprint),
        Vec::new(),
        vec![sql],
    )
}

fn plan_clickhouse_view_edit(
    request: crate::db::catalog_mutation::CatalogMutationRequest,
    draft: crate::model::catalog_editor::CatalogDraft,
    baseline: Option<crate::db::catalog_mutation::CatalogObjectDefinition>,
    current_database: &str,
) -> Result<CatalogMutationPlan, CatalogMutationError> {
    use crate::db::catalog_mutation::CatalogObjectDefinition;
    use crate::model::catalog_editor::CatalogDraft;
    let Some(CatalogObjectDefinition::View(definition)) = baseline else {
        return Err(CatalogMutationError::InvalidDraft {
            reason: "ClickHouse view edit requires a native baseline".into(),
        });
    };
    let Some(native) = definition.clickhouse_native else {
        return Err(CatalogMutationError::InvalidDraft {
            reason: "ClickHouse view edit requires canonical native DDL".into(),
        });
    };
    let CatalogDraft::View(draft) = draft else {
        return Err(CatalogMutationError::InvalidDraft {
            reason: "ClickHouse view draft required".into(),
        });
    };
    draft.validate()?;
    let CatalogMutationAnchor::Catalog(object) = &request.anchor else {
        return Err(CatalogMutationError::InvalidAnchor {
            reason: "ClickHouse view edit requires exact object identity",
        });
    };
    if request.object_type != CatalogObjectType::Catalog(CatalogKind::View)
        || object.kind != CatalogKind::View
        || object.native_path.len() != 3
        || object.native_path[0] != native.database
        || object.native_path[2] != native.name
        || request.current_database.as_deref() != Some(current_database)
    {
        return Err(CatalogMutationError::StaleState);
    }
    if draft.schema.value() != native.database
        || draft.owner.value() != ""
        || draft.query.value() != native.query
        || draft.clickhouse_native.as_ref() != Some(&native)
        || !draft.output_columns.value().is_empty()
        || draft.security_barrier.value.is_some()
        || draft.security_invoker.value.is_some()
        || draft.check_option.value.is_some()
    {
        return Err(CatalogMutationError::InvalidDraft { reason: "ClickHouse view edits preserve query and native clauses; only rename or comment is supported".into() });
    }
    let changed_name = draft.name.value() != native.name;
    if !draft.comment.value().is_empty()
        && matches!(
            native.comment,
            crate::db::catalog::OptionalMetadata::Unsupported
        )
    {
        return Err(CatalogMutationError::InvalidDraft {
            reason: "ClickHouse view comments are unsupported by server metadata".into(),
        });
    }
    let changed_comment = match &native.comment {
        crate::db::catalog::OptionalMetadata::Unsupported => !draft.comment.value().is_empty(),
        crate::db::catalog::OptionalMetadata::Supported(value) => {
            draft.comment.value() != value.as_deref().unwrap_or_default()
        }
    };
    if usize::from(changed_name) + usize::from(changed_comment) != 1 {
        return Err(CatalogMutationError::InvalidDraft {
            reason: "ClickHouse view edit supports one rename or comment change at a time".into(),
        });
    }
    let (sql, new_name) = if changed_name {
        (
            format!(
                "RENAME TABLE {}.{} TO {}.{}",
                quote_identifier(&native.database),
                quote_identifier(&native.name),
                quote_identifier(&native.database),
                quote_identifier(draft.name.value().trim())
            ),
            draft.name.value().trim().to_owned(),
        )
    } else {
        if !matches!(
            native.comment,
            crate::db::catalog::OptionalMetadata::Supported(_)
        ) {
            return Err(CatalogMutationError::InvalidDraft {
                reason: "ClickHouse view comment metadata is unavailable".into(),
            });
        }
        let escaped = draft
            .comment
            .value()
            .replace('\\', "\\\\")
            .replace('\'', "\\'");
        (
            format!(
                "ALTER TABLE {}.{} MODIFY COMMENT '{}'",
                quote_identifier(&native.database),
                quote_identifier(&native.name),
                escaped
            ),
            native.name.clone(),
        )
    };
    let schema = CatalogId::new(
        request.connection.profile_id,
        CatalogKind::Schema,
        [native.database.as_str(), native.database.as_str()],
    );
    let refresh = CatalogTarget::objects(schema, crate::db::catalog::ObjectGroup::Views).map_err(
        |error| CatalogMutationError::InvalidPlan {
            reason: error.to_string(),
        },
    )?;
    let selection = CatalogSelectionHint::Object(CatalogId::new(
        request.connection.profile_id,
        CatalogKind::View,
        [
            native.database.as_str(),
            native.database.as_str(),
            new_name.as_str(),
        ],
    ));
    CatalogMutationPlan::new(
        request.clone(),
        request.object_type,
        CatalogMutationExecutionMode::Autocommit,
        CatalogMutationTarget::maintenance(native.database.clone())?,
        vec![refresh],
        selection,
        Some(definition.baseline_fingerprint),
        Vec::new(),
        vec![sql],
    )
}

fn plan_clickhouse_materialized_view_edit(
    request: crate::db::catalog_mutation::CatalogMutationRequest,
    _draft: crate::model::catalog_editor::CatalogDraft,
    _baseline: Option<crate::db::catalog_mutation::CatalogObjectDefinition>,
    _current_database: &str,
) -> Result<CatalogMutationPlan, CatalogMutationError> {
    Err(CatalogMutationError::UnsupportedOperation {
        object_type: request.object_type,
    })
}

fn safe_view_query(query: &str) -> bool {
    let query = query.trim();
    !query.is_empty()
        && !query.contains(';')
        && !query.contains("--")
        && !query.contains("/*")
        && !query.contains("*/")
}

fn safe_clickhouse_type(value: &str) -> bool {
    !value.trim().is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '_' | '(' | ')' | ',' | ' ' | '\'' | '[' | ']')
        })
        && !value.contains(';')
        && !value.contains("--")
        && !value.contains("/*")
}

fn relation_names(id: &CatalogId) -> Result<(&str, &str), CatalogDropError> {
    if id.native_path.len() != 3 {
        return Err(CatalogDropError::InvalidMetadata {
            kind: id.kind,
            reason: "ClickHouse relation identity must contain database, namespace, object".into(),
        });
    }
    Ok((&id.native_path[0], &id.native_path[2]))
}

fn quote_identifier(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

fn clickhouse_order_by_clause(ddl: &str) -> Option<String> {
    let upper = ddl.to_ascii_uppercase();
    let marker = upper.find(" ORDER BY ")? + " ORDER BY ".len();
    let tail = &ddl[marker..];
    let upper_tail = tail.to_ascii_uppercase();
    let end = [
        " PARTITION BY ",
        " PRIMARY KEY ",
        " SAMPLE BY ",
        " TTL ",
        " SETTINGS ",
        " COMMENT ",
    ]
    .iter()
    .filter_map(|clause| upper_tail.find(clause))
    .min()
    .unwrap_or(tail.len());
    Some(tail[..end].trim().to_owned()).filter(|value| !value.is_empty())
}
