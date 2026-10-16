use crate::db::catalog::{CatalogCapabilities, ColumnMetadataCapabilities, NamespaceModel};
use crate::db::{
    DatabaseError, ErrorCategory,
    catalog::{
        CatalogCount, CatalogCursor, CatalogEntry, CatalogGroupSummary, CatalogId, CatalogKind,
        CatalogPage, CatalogRequest, CatalogTarget, ColumnMetadata, ObjectGroup, OptionalMetadata,
        QualifiedName,
    },
    value::CellValue,
};
use crate::security::sanitize_terminal_text;

use super::transport::ClickHouseAdapter;

impl ClickHouseAdapter {
    pub fn catalog_capabilities() -> CatalogCapabilities {
        CatalogCapabilities {
            namespace_model: NamespaceModel::DatabaseIsSchema,
            top_level_groups: vec![
                ObjectGroup::Tables,
                ObjectGroup::Views,
                ObjectGroup::MaterializedViews,
                ObjectGroup::Dictionaries,
                ObjectGroup::UserDefinedFunctions,
                ObjectGroup::Functions,
            ],
            column_metadata: ColumnMetadataCapabilities {
                default_expression: true,
                comment: true,
                ..Default::default()
            },
            supports_lazy_children: true,
        }
    }

    pub async fn load_catalog_page(
        &self,
        request: &CatalogRequest,
    ) -> Result<CatalogPage, DatabaseError> {
        request
            .validate_for_profile(self.profile_id())
            .map_err(|error| catalog_error(error.to_string()))?;
        if request.page_size > crate::db::catalog::MAX_CATALOG_PAGE_SIZE {
            return Err(catalog_error(
                "ClickHouse catalog page size exceeds the supported limit",
            ));
        }
        let profile_id = request.key.connection.profile_id;
        let scope = &request.scope;
        let mut pending_page_cursor = None;
        let mut pending_page_has_more = false;
        let mut entries = match &request.key.target {
            CatalogTarget::Databases => {
                let cursor = request
                    .key
                    .cursor
                    .as_ref()
                    .map(|cursor| cursor.keyset_parts())
                    .transpose()
                    .map_err(|error| catalog_error(error.to_string()))?;
                let mut sql = "SELECT name FROM system.databases WHERE name NOT IN ('system', 'information_schema', 'INFORMATION_SCHEMA')".to_owned();
                let mut params = Vec::new();
                let mut selected_params = Vec::new();
                if let crate::profile::CatalogSelection::Selected(databases) = &scope.databases {
                    if databases.is_empty() {
                        return CatalogPage::new(request, Vec::new(), CatalogCount::Exact(0), None)
                            .map_err(|error| catalog_error(error.to_string()));
                    }
                    sql.push_str(" AND name IN (");
                    sql.push_str(
                        &std::iter::repeat_n("?", databases.len())
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                    sql.push(')');
                    selected_params.extend(databases.iter().map(|database| database.name.clone()));
                    params.extend(selected_params.iter().map(String::as_str));
                }
                if let Some((sort, tie)) = cursor {
                    sql.push_str(" AND (lower(name) > ? OR (lower(name) = ? AND name > ?))");
                    params.extend([sort, sort, tie]);
                }
                sql.push_str(" ORDER BY lower(name), name LIMIT ");
                sql.push_str(&(request.page_size + 1).to_string());
                let rows = self.catalog_rows(&sql, &params).await?;
                let names = rows
                    .iter()
                    .map(|row| text(row, 0).map(str::to_owned))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut names = names;
                let has_more = names.len() > request.page_size;
                let cursor_name = has_more.then(|| names[request.page_size - 1].clone());
                names.truncate(request.page_size);
                let result = names
                    .into_iter()
                    .map(|name| {
                        let id = CatalogId::new(profile_id, CatalogKind::Database, [name.clone()]);
                        CatalogEntry::database(
                            id,
                            QualifiedName {
                                database: Some(name.clone()),
                                schema: None,
                                object: name,
                            },
                            "DATABASE",
                            OptionalMetadata::Unsupported,
                            true,
                        )
                        .map_err(|error| catalog_error(error.to_string()))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                pending_page_cursor = cursor_name
                    .map(|name| CatalogCursor::from_keyset(&name.to_lowercase(), &name))
                    .transpose()
                    .map_err(|error| catalog_error(error.to_string()))?;
                pending_page_has_more = has_more;
                result
            }
            CatalogTarget::Schemas { database } => {
                let name = database
                    .native_path
                    .first()
                    .ok_or_else(|| catalog_error("database target has no database name"))?;
                if database.connection_id != profile_id {
                    return Err(catalog_error(
                        "database target belongs to a different profile",
                    ));
                }
                if !scope.allows_database(name) {
                    return Err(catalog_error(
                        "database target is outside the selected catalog scope",
                    ));
                }
                vec![
                    CatalogEntry::schema(
                        CatalogId::new(
                            profile_id,
                            CatalogKind::Schema,
                            [name.clone(), name.clone()],
                        ),
                        database.clone(),
                        QualifiedName {
                            database: Some(name.clone()),
                            schema: Some(name.clone()),
                            object: name.clone(),
                        },
                        "DATABASE_NAMESPACE",
                        OptionalMetadata::Unsupported,
                        true,
                    )
                    .map_err(|error| catalog_error(error.to_string()))?,
                ]
            }
            CatalogTarget::Groups { schema } => {
                if schema.connection_id != profile_id {
                    return Err(catalog_error(
                        "schema target belongs to a different profile",
                    ));
                }
                let (database, _) = schema_names(schema)?;
                if !scope.allows_schema(database, database) {
                    return Err(catalog_error(
                        "database target is outside the selected catalog scope",
                    ));
                }
                let group_summaries = self.group_summaries(database).await?;
                return CatalogPage::groups(request, group_summaries, CatalogCount::Unknown, None)
                    .map_err(|error| catalog_error(error.to_string()));
            }
            CatalogTarget::Objects { schema, group } => {
                if schema.connection_id != profile_id {
                    return Err(catalog_error(
                        "schema target belongs to a different profile",
                    ));
                }
                let (database, _) = schema_names(schema)?;
                if !scope.allows_schema(database, database) {
                    return Err(catalog_error(
                        "database target is outside the selected catalog scope",
                    ));
                }
                let Some((kind, query)) = object_query(*group) else {
                    return CatalogPage::new(request, Vec::new(), CatalogCount::Exact(0), None)
                        .map_err(|error| catalog_error(error.to_string()));
                };
                let cursor_key = request
                    .key
                    .cursor
                    .as_ref()
                    .map(|cursor| cursor.keyset_parts())
                    .transpose()
                    .map_err(|error| catalog_error(error.to_string()))?;
                let mut effective_query = query.to_owned();
                let mut params = vec![database];
                if let Some((sort, tie)) = cursor_key {
                    let Some((where_clause, order_clause)) =
                        effective_query.split_once(" ORDER BY ")
                    else {
                        return Err(catalog_error("catalog query is missing its stable sort"));
                    };
                    if order_clause.trim().is_empty() {
                        return Err(catalog_error("catalog query has an empty stable sort"));
                    }
                    let with_cursor = format!(
                        "{where_clause} AND (lower(name) > ? OR (lower(name) = ? AND name > ?)) ORDER BY {order_clause}"
                    );
                    effective_query = with_cursor;
                    params.extend([sort, sort, tie]);
                }
                effective_query.push_str(" LIMIT ");
                effective_query.push_str(&(request.page_size + 1).to_string());
                let rows = self.catalog_rows(&effective_query, &params).await?;
                let has_more = rows.len() > request.page_size;
                pending_page_has_more = has_more;
                let cursor_row = has_more.then(|| &rows[request.page_size - 1]);
                pending_page_cursor = cursor_row
                    .map(|row| {
                        let name = text(row, 0)?;
                        CatalogCursor::from_keyset(&name.to_lowercase(), name)
                            .map_err(|error| catalog_error(error.to_string()))
                    })
                    .transpose()?;
                let mut rows = rows;
                rows.truncate(request.page_size);
                rows.into_iter()
                    .map(|row| {
                        let name = text(&row, 0)?;
                        let native_kind = text(&row, 1)?;
                        let comment = text(&row, 2).unwrap_or_default();
                        let id = CatalogId::new(
                            profile_id,
                            kind,
                            [database.to_owned(), database.to_owned(), name.to_owned()],
                        );
                        let qualified = QualifiedName {
                            database: Some(database.to_owned()),
                            schema: Some(database.to_owned()),
                            object: name.to_owned(),
                        };
                        if matches!(
                            kind,
                            CatalogKind::Dictionary | CatalogKind::UserDefinedFunction
                        ) {
                            CatalogEntry::object(
                                id,
                                schema.clone(),
                                qualified,
                                native_kind.to_owned(),
                                OptionalMetadata::Unsupported,
                                false,
                            )
                        } else {
                            CatalogEntry::relation(
                                id,
                                schema.clone(),
                                qualified,
                                native_kind.to_owned(),
                                OptionalMetadata::Supported(
                                    (!comment.is_empty()).then(|| comment.to_owned()),
                                ),
                                true,
                            )
                        }
                        .map_err(|error| catalog_error(error.to_string()))
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
            CatalogTarget::RelationChildren { relation } => {
                if relation.connection_id != profile_id {
                    return Err(catalog_error(
                        "relation target belongs to a different profile",
                    ));
                }
                let (database, table) = relation_names(relation)?;
                if !scope.allows_schema(database, database) {
                    return Err(catalog_error(
                        "relation target is outside the selected catalog scope",
                    ));
                }
                let cursor = request
                    .key
                    .cursor
                    .as_ref()
                    .map(|cursor| cursor.keyset_parts())
                    .transpose()
                    .map_err(|error| catalog_error(error.to_string()))?;
                let mut sql = "SELECT name, type, position, default_kind, default_expression, comment, data_compressed_bytes, data_uncompressed_bytes, marks_bytes FROM system.columns WHERE database = ? AND table = ?".to_owned();
                let mut owned_params = Vec::new();
                let mut params = vec![database, table];
                if let Some((sort, tie)) = cursor {
                    let position = sort
                        .parse::<u64>()
                        .map_err(|_| catalog_error("invalid column cursor position"))?
                        .to_string();
                    sql.push_str(
                        " AND (position > toUInt64(?) OR (position = toUInt64(?) AND name >= ?))",
                    );
                    owned_params.extend([position.clone(), position, tie.to_owned()]);
                    let len = owned_params.len();
                    params.extend(owned_params[len - 3..].iter().map(String::as_str));
                }
                sql.push_str(" ORDER BY position, name LIMIT ");
                sql.push_str(&(request.page_size + 1).to_string());
                let rows = self.catalog_rows(&sql, &params).await?;
                let has_more = rows.len() > request.page_size;
                let cursor_row = has_more.then(|| &rows[request.page_size - 1]);
                pending_page_has_more = has_more;
                pending_page_cursor = cursor_row
                    .map(|row| {
                        CatalogCursor::from_keyset(
                            &format!("{:020}", unsigned(row, 2)?),
                            text(row, 0)?,
                        )
                        .map_err(|error| catalog_error(error.to_string()))
                    })
                    .transpose()?;
                rows
                    .into_iter()
                    .take(request.page_size)
                    .map(|row| {
                        let name = text(&row, 0)?;
                        let native_type = text(&row, 1)?;
                        let ordinal = unsigned(&row, 2)?.min(u32::MAX as u64) as u32;
                        let default_kind = text(&row, 3)?;
                        let default_expression = text(&row, 4)?;
                        let comment = text(&row, 5)?;
                        let data_compressed = unsigned(&row, 6)?;
                        let data_uncompressed = unsigned(&row, 7)?;
                        let marks_bytes = unsigned(&row, 8)?;
                        let metadata =
                            crate::db::catalog::CatalogMetadata::Column(ColumnMetadata {
                                ordinal_position: ordinal,
                                native_type: native_type.to_owned(),
                                type_family: OptionalMetadata::Unsupported,
                                nullable: native_type.starts_with("Nullable("),
                                default_expression: if default_kind.is_empty() {
                                    OptionalMetadata::Supported(None)
                                } else {
                                    OptionalMetadata::Supported(Some(format!(
                                        "{default_kind} {default_expression}"
                                    )))
                                },
                                comment: OptionalMetadata::Supported(
                                    (!comment.is_empty()).then(|| comment.to_owned()),
                                ),
                                storage_statistics: OptionalMetadata::Supported(Some(format!(
                                    "compressed_bytes={data_compressed}; uncompressed_bytes={data_uncompressed}; marks_bytes={marks_bytes}"
                                ))),
                                identity: OptionalMetadata::Supported(None),
                                auto_increment: OptionalMetadata::Supported(None),
                                generated_expression: OptionalMetadata::Supported(None),
                                hidden: OptionalMetadata::Supported(None),
                                numeric_precision: OptionalMetadata::Unsupported,
                                numeric_scale: OptionalMetadata::Unsupported,
                                character_maximum_length: OptionalMetadata::Unsupported,
                                collation: OptionalMetadata::Unsupported,
                                character_set: OptionalMetadata::Unsupported,
                                constraint_memberships: Vec::new(),
                            });
                        CatalogEntry::relation_child(
                            CatalogId::new(
                                profile_id,
                                CatalogKind::Column,
                                [
                                    database.to_owned(),
                                    database.to_owned(),
                                    table.to_owned(),
                                    name.to_owned(),
                                ],
                            ),
                            relation.clone(),
                            QualifiedName {
                                database: Some(database.to_owned()),
                                schema: Some(database.to_owned()),
                                object: name.to_owned(),
                            },
                            native_type.to_owned(),
                            OptionalMetadata::Supported(
                                (!comment.is_empty()).then(|| comment.to_owned()),
                            ),
                            metadata,
                        )
                        .map_err(|error| catalog_error(error.to_string()))
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
            CatalogTarget::ProjectionChildren { table: relation } => {
                let (database, table) = relation_names(relation)?;
                if relation.connection_id != profile_id || relation.kind != CatalogKind::Table {
                    return Err(catalog_error(
                        "projection target must be a table from this profile",
                    ));
                }
                if !scope.allows_schema(database, database) {
                    return Err(catalog_error(
                        "projection target is outside the selected catalog scope",
                    ));
                }
                let cursor = request
                    .key
                    .cursor
                    .as_ref()
                    .map(CatalogCursor::keyset_parts)
                    .transpose()
                    .map_err(|error| catalog_error(error.to_string()))?;
                let mut sql = "SELECT name, sorting_key FROM system.projections WHERE database = ? AND table = ?".to_owned();
                let mut owned_params = vec![database.to_owned(), table.to_owned()];
                if let Some((sort, tie)) = cursor {
                    sql.push_str(" AND (lower(name) > ? OR (lower(name) = ? AND name > ?))");
                    owned_params.extend([sort.to_owned(), sort.to_owned(), tie.to_owned()]);
                }
                sql.push_str(" ORDER BY lower(name), name");
                sql.push_str(" LIMIT ");
                sql.push_str(&(request.page_size + 1).to_string());
                let params = owned_params.iter().map(String::as_str).collect::<Vec<_>>();
                let mut rows = self.catalog_rows(&sql, &params).await?;
                let has_more = rows.len() > request.page_size;
                let next_cursor = has_more
                    .then(|| {
                        let row = &rows[request.page_size - 1];
                        let name = text(row, 0)?;
                        CatalogCursor::from_keyset(&name.to_lowercase(), name)
                            .map_err(|error| catalog_error(error.to_string()))
                    })
                    .transpose()?;
                rows.truncate(request.page_size);
                let entries = rows
                    .into_iter()
                    .map(|row| {
                        let name = text(&row, 0)?.to_owned();
                        let sorting_key = text(&row, 1)?.to_owned();
                        let id = CatalogId::new(
                            profile_id,
                            CatalogKind::Projection,
                            [
                                database.to_owned(),
                                database.to_owned(),
                                table.to_owned(),
                                name.clone(),
                            ],
                        );
                        CatalogEntry::projection(
                            id,
                            relation.clone(),
                            QualifiedName {
                                database: Some(database.to_owned()),
                                schema: Some(database.to_owned()),
                                object: name,
                            },
                            "PROJECTION",
                            crate::db::catalog::IndexMetadata {
                                columns: sorting_key
                                    .split(',')
                                    .map(str::trim)
                                    .filter(|part| !part.is_empty())
                                    .map(str::to_owned)
                                    .collect(),
                                unique: false,
                            },
                        )
                        .map_err(|error| catalog_error(error.to_string()))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                pending_page_cursor = next_cursor;
                pending_page_has_more = has_more;
                entries
            }
        };
        let next_cursor = if let Some(cursor) = pending_page_cursor {
            Some(cursor)
        } else if entries.len() > request.page_size {
            let entry = &entries[request.page_size - 1];
            Some(
                CatalogCursor::from_keyset(
                    &entry.qualified_name.object,
                    &entry.qualified_name.object,
                )
                .map_err(|error| catalog_error(error.to_string()))?,
            )
        } else {
            None
        };
        let next_cursor = if pending_page_has_more {
            next_cursor
        } else {
            None
        };
        entries.truncate(request.page_size);
        CatalogPage::new(request, entries, CatalogCount::Unknown, next_cursor)
            .map_err(|error| catalog_error(error.to_string()))
    }

    async fn group_summaries(
        &self,
        database: &str,
    ) -> Result<Vec<CatalogGroupSummary>, DatabaseError> {
        let rows = self.catalog_rows(
            "SELECT countIf(engine NOT IN ('View', 'MaterializedView', 'Dictionary')) AS tables, countIf(engine = 'View') AS views, countIf(engine = 'MaterializedView') AS materialized_views FROM system.tables WHERE database = ? AND name NOT LIKE '.inner%'",
            &[database],
        ).await?;
        let row = rows
            .first()
            .ok_or_else(|| catalog_error("ClickHouse catalog group query returned no row"))?;
        let mut summaries = vec![
            (ObjectGroup::Tables, unsigned(row, 0)?),
            (ObjectGroup::Views, unsigned(row, 1)?),
            (ObjectGroup::MaterializedViews, unsigned(row, 2)?),
        ]
        .into_iter()
        .map(|(group, count)| CatalogGroupSummary {
            group,
            object_count: CatalogCount::Exact(count),
        })
        .collect::<Vec<_>>();
        let functions = self
            .catalog_rows("SELECT count() FROM system.functions", &[])
            .await?;
        summaries.push(CatalogGroupSummary {
            group: ObjectGroup::Functions,
            object_count: CatalogCount::Exact(unsigned(
                functions
                    .first()
                    .ok_or_else(|| catalog_error("function count query returned no row"))?,
                0,
            )?),
        });
        let dictionaries = self
            .catalog_rows(
                "SELECT count() FROM system.dictionaries WHERE database = ?",
                &[database],
            )
            .await?;
        summaries.push(CatalogGroupSummary {
            group: ObjectGroup::Dictionaries,
            object_count: CatalogCount::Exact(unsigned(
                dictionaries
                    .first()
                    .ok_or_else(|| catalog_error("dictionary count query returned no row"))?,
                0,
            )?),
        });
        let functions = self.catalog_rows(
            "SELECT count() FROM system.functions WHERE is_aggregate = 0 AND origin = 'SQLUserDefined'",
            &[],
        ).await?;
        summaries.push(CatalogGroupSummary {
            group: ObjectGroup::UserDefinedFunctions,
            object_count: CatalogCount::Exact(unsigned(
                functions
                    .first()
                    .ok_or_else(|| catalog_error("ClickHouse UDF count query returned no row"))?,
                0,
            )?),
        });
        Ok(summaries)
    }

    pub(super) async fn catalog_rows(
        &self,
        sql: &str,
        params: &[&str],
    ) -> Result<Vec<Vec<CellValue>>, DatabaseError> {
        let sql = bind_catalog_params(sql, params)?;
        let outcome = self.execute(&sql).await?;
        Ok(outcome
            .result_sets
            .into_iter()
            .next()
            .map(|set| set.rows)
            .unwrap_or_default())
    }

    pub async fn search_catalog(
        &self,
        request: &crate::db::catalog::CatalogSearchRequest,
    ) -> Result<crate::db::catalog::CatalogSearchPage, DatabaseError> {
        request
            .validate()
            .map_err(|error| catalog_error(error.to_string()))?;
        if request.connection.profile_id != self.profile_id() {
            return Err(catalog_error(
                "catalog search connection identity does not match this ClickHouse profile",
            ));
        }
        if !self.catalog_scope.allows_database(self.server_database()) {
            return Err(catalog_error(
                "ClickHouse connected database is outside the profile catalog scope",
            ));
        }
        let pattern = format!(
            "%{}%",
            request
                .query
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let mut sql = "SELECT database, name, native_kind, comment FROM (SELECT database, name, engine AS native_kind, comment FROM system.tables UNION ALL SELECT database, name, 'DICTIONARY' AS native_kind, '' AS comment FROM system.dictionaries UNION ALL SELECT database, name, 'SQLUserDefined' AS native_kind, '' AS comment FROM system.functions WHERE is_aggregate = 0 AND origin = 'SQLUserDefined') WHERE (name ILIKE ? ESCAPE '\\\\' OR comment ILIKE ? ESCAPE '\\\\')".to_owned();
        let mut params = vec![pattern.as_str(), pattern.as_str()];
        let mut selected_params = Vec::new();
        match &request.scope.databases {
            crate::profile::CatalogSelection::Selected(databases) if databases.is_empty() => {
                return crate::db::catalog::CatalogSearchPage::new(
                    request,
                    Vec::new(),
                    Some(0),
                    false,
                )
                .map_err(|error| catalog_error(error.to_string()));
            }
            crate::profile::CatalogSelection::Selected(databases) => {
                sql.push_str(" AND (");
                let mut clauses = Vec::new();
                for database in databases {
                    match &database.schemas {
                        crate::profile::CatalogSelection::All => {
                            clauses.push("database = ?");
                            selected_params.push(database.name.clone());
                        }
                        crate::profile::CatalogSelection::Selected(schemas)
                            if schemas.iter().any(|schema| schema == &database.name) =>
                        {
                            clauses.push("database = ?");
                            selected_params.push(database.name.clone());
                        }
                        crate::profile::CatalogSelection::Selected(_) => {}
                    }
                }
                if clauses.is_empty() {
                    sql.push('0');
                } else {
                    sql.push_str(&clauses.join(" OR "));
                }
                sql.push(')');
                params.extend(selected_params.iter().map(String::as_str));
            }
            crate::profile::CatalogSelection::All => {}
        }
        if request.object_scope == crate::db::catalog::CatalogSearchObjectScope::RelationsOnly {
            sql.push_str(" AND native_kind IN ('MergeTree', 'ReplacingMergeTree', 'SummingMergeTree', 'AggregatingMergeTree', 'CollapsingMergeTree', 'VersionedCollapsingMergeTree', 'GraphiteMergeTree', 'Log', 'TinyLog', 'StripeLog', 'Memory', 'Buffer', 'Distributed', 'View', 'MaterializedView', 'LiveView', 'WindowView', 'URL', 'File', 'Null', 'Set', 'Join')");
        }
        // The bounded extra row lets the caller distinguish an exhausted page
        // from a result set truncated by the UI limit.
        sql.push_str(" ORDER BY database, lower(name), name LIMIT ");
        sql.push_str(&(request.limit + 1).to_string());
        let rows = self.catalog_rows(&sql, &params).await?;
        let truncated = rows.len() > request.limit;
        let mut hits = Vec::new();
        for row in rows.into_iter().take(request.limit) {
            let database = text(&row, 0)?.to_owned();
            let name = text(&row, 1)?.to_owned();
            let native_kind = text(&row, 2)?.to_owned();
            let comment = text(&row, 3)?.to_owned();
            let kind = match native_kind.as_str() {
                "DICTIONARY" | "Dictionary" => CatalogKind::Dictionary,
                "SQLUserDefined" => CatalogKind::UserDefinedFunction,
                "View" => CatalogKind::View,
                "MaterializedView" => CatalogKind::MaterializedView,
                _ => CatalogKind::Table,
            };
            if !request.object_scope.includes(kind) {
                continue;
            }
            let hit_database = database.clone();
            let hit_database_id = CatalogId::new(
                request.connection.profile_id,
                CatalogKind::Database,
                [hit_database.clone()],
            );
            let schema_id = CatalogId::new(
                request.connection.profile_id,
                CatalogKind::Schema,
                [hit_database.clone(), hit_database.clone()],
            );
            let database_entry = CatalogEntry::database(
                hit_database_id.clone(),
                QualifiedName {
                    database: Some(hit_database.clone()),
                    schema: None,
                    object: hit_database.clone(),
                },
                "DATABASE",
                OptionalMetadata::Unsupported,
                true,
            )
            .map_err(|error| catalog_error(error.to_string()))?;
            let schema = CatalogEntry::schema(
                schema_id,
                hit_database_id,
                QualifiedName {
                    database: Some(hit_database.clone()),
                    schema: Some(hit_database.clone()),
                    object: hit_database.clone(),
                },
                "DATABASE_NAMESPACE",
                OptionalMetadata::Unsupported,
                true,
            )
            .map_err(|error| catalog_error(error.to_string()))?;
            let id = CatalogId::new(
                request.connection.profile_id,
                kind,
                [hit_database.clone(), hit_database.clone(), name.clone()],
            );
            let qualified = QualifiedName {
                database: Some(hit_database.clone()),
                schema: Some(hit_database),
                object: name,
            };
            let entry = if matches!(
                kind,
                CatalogKind::Dictionary | CatalogKind::UserDefinedFunction
            ) {
                CatalogEntry::object(
                    id,
                    schema.id.clone(),
                    qualified,
                    native_kind,
                    OptionalMetadata::Unsupported,
                    false,
                )
            } else {
                CatalogEntry::relation(
                    id,
                    schema.id.clone(),
                    qualified,
                    native_kind,
                    OptionalMetadata::Supported((!comment.is_empty()).then_some(comment)),
                    true,
                )
            }
            .map_err(|error| catalog_error(error.to_string()))?;
            hits.push(crate::db::catalog::CatalogSearchHit {
                entry,
                ancestors: vec![database_entry, schema],
            });
        }
        crate::db::catalog::CatalogSearchPage::new(request, hits, None, truncated)
            .map_err(|error| catalog_error(error.to_string()))
    }
}

fn bind_catalog_params(sql: &str, params: &[&str]) -> Result<String, DatabaseError> {
    let mut output = String::with_capacity(sql.len());
    let mut params = params.iter();
    let mut quote = None;
    let mut index = 0;
    let bytes = sql.as_bytes();
    while index < bytes.len() {
        match bytes[index] {
            quote_byte @ (b'\'' | b'`') => {
                output.push(quote_byte as char);
                if quote == Some(quote_byte) && bytes.get(index + 1) == Some(&quote_byte) {
                    output.push(quote_byte as char);
                    index += 2;
                    continue;
                }
                if quote == Some(quote_byte) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(quote_byte);
                }
                index += 1;
            }
            b'\\' if quote == Some(b'\'') || quote == Some(b'`') => {
                output.push('\\');
                if let Some(next) = bytes.get(index + 1) {
                    output.push(*next as char);
                }
                index += 2;
            }
            b'-' if quote.is_none() && bytes.get(index + 1) == Some(&b'-') => {
                while index < bytes.len() {
                    let byte = bytes[index];
                    output.push(byte as char);
                    index += 1;
                    if byte == b'\n' {
                        break;
                    }
                }
            }
            b'/' if quote.is_none() && bytes.get(index + 1) == Some(&b'*') => {
                while index < bytes.len() {
                    output.push(bytes[index] as char);
                    if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                        output.push('/');
                        index += 2;
                        break;
                    }
                    index += 1;
                }
            }
            b'?' if quote.is_none() => {
                let value = params
                    .next()
                    .ok_or_else(|| catalog_error("catalog query parameter count mismatch"))?;
                output.push('\'');
                output.push_str(&value.replace('\\', "\\\\").replace('\'', "\\'"));
                output.push('\'');
                index += 1;
            }
            byte => {
                output.push(byte as char);
                index += 1;
            }
        }
    }
    if params.next().is_some() {
        return Err(catalog_error("catalog query parameter count mismatch"));
    }
    Ok(output)
}

fn schema_names(id: &CatalogId) -> Result<(&str, &str), DatabaseError> {
    if id.native_path.len() != 2 {
        return Err(catalog_error(
            "ClickHouse schema identity must contain database and namespace",
        ));
    }
    Ok((&id.native_path[0], &id.native_path[1]))
}

fn relation_names(id: &CatalogId) -> Result<(&str, &str), DatabaseError> {
    if id.native_path.len() < 3 {
        return Err(catalog_error(
            "relation identity must contain database and object names",
        ));
    }
    Ok((&id.native_path[0], &id.native_path[2]))
}

fn text(row: &[CellValue], index: usize) -> Result<&str, DatabaseError> {
    match row.get(index) {
        Some(CellValue::Text(value)) => Ok(value),
        Some(CellValue::Null) | None => Ok(""),
        _ => Err(catalog_error(
            "unexpected non-text value in ClickHouse catalog response",
        )),
    }
}

fn unsigned(row: &[CellValue], index: usize) -> Result<u64, DatabaseError> {
    match row.get(index) {
        Some(CellValue::Unsigned(value)) => Ok(*value),
        Some(CellValue::Integer(value)) if *value >= 0 => Ok(*value as u64),
        Some(CellValue::Text(value)) => value
            .parse()
            .map_err(|_| catalog_error("invalid numeric catalog value")),
        _ => Ok(0),
    }
}

fn object_query(group: ObjectGroup) -> Option<(CatalogKind, &'static str)> {
    match group {
        ObjectGroup::Tables => Some((
            CatalogKind::Table,
            "SELECT name, engine, comment FROM system.tables WHERE database = ? AND engine NOT IN ('View', 'MaterializedView', 'Dictionary') AND name NOT LIKE '.inner%' ORDER BY lower(name), name",
        )),
        ObjectGroup::Views => Some((
            CatalogKind::View,
            "SELECT name, engine, comment FROM system.tables WHERE database = ? AND engine = 'View' ORDER BY lower(name), name",
        )),
        ObjectGroup::MaterializedViews => Some((
            CatalogKind::MaterializedView,
            "SELECT name, engine, comment FROM system.tables WHERE database = ? AND engine = 'MaterializedView' ORDER BY lower(name), name",
        )),
        ObjectGroup::Dictionaries => Some((
            CatalogKind::Dictionary,
            "SELECT name, 'DICTIONARY', '' FROM system.dictionaries WHERE database = ? ORDER BY lower(name), name",
        )),
        ObjectGroup::UserDefinedFunctions => Some((
            CatalogKind::UserDefinedFunction,
            "SELECT name, 'SQLUserDefined', '' FROM system.functions WHERE database = ? AND is_aggregate = 0 AND origin = 'SQLUserDefined' ORDER BY lower(name), name",
        )),
        ObjectGroup::Functions => Some((
            CatalogKind::Function,
            "SELECT name, 'FUNCTION', '' FROM system.functions WHERE 1 = 1 ORDER BY lower(name), name",
        )),
        _ => None,
    }
}

impl ClickHouseAdapter {
    pub async fn load_projection_page(
        &self,
        request: &CatalogRequest,
    ) -> Result<CatalogPage, DatabaseError> {
        self.load_catalog_page(request).await
    }
}

fn catalog_error(message: impl AsRef<str>) -> DatabaseError {
    DatabaseError {
        category: ErrorCategory::Sql,
        code: Some("clickhouse_catalog".into()),
        message: sanitize_terminal_text(message.as_ref()),
        diagnostic: None,
    }
}
