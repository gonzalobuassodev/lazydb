use thiserror::Error;
use uuid::Uuid;

use crate::{
    db::catalog::{CatalogEntry, CatalogId, CatalogKind},
    identity::ConnectionIdentity,
};

/// The catalog identity used to build a drop plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogDropRequest {
    pub connection: ConnectionIdentity,
    pub request_id: u64,
    pub catalog_epoch: u64,
    pub object: CatalogId,
    pub entry: Option<CatalogEntry>,
    pub maintenance_database: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CatalogDropExecutionTarget {
    CurrentConnection,
    MaintenanceDatabase(String),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CatalogDropBehavior {
    #[default]
    Restrict,
    Cascade,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum CatalogDropError {
    #[error("catalog drop request has an empty object ID")]
    EmptyObjectId,
    #[error(
        "catalog drop request profile {object_profile_id} does not match connection profile {connection_profile_id}"
    )]
    ProfileMismatch {
        object_profile_id: Uuid,
        connection_profile_id: Uuid,
    },
    #[error("catalog drop SQL is empty")]
    EmptySql,
    #[error("catalog drop SQL contains a forbidden clause")]
    UnsafeSql,
    #[error("catalog drop entry kind {entry_kind:?} does not match its ID kind {id_kind:?}")]
    KindMismatch {
        id_kind: CatalogKind,
        entry_kind: CatalogKind,
    },
    #[error("catalog drop entry ID does not match the request")]
    ObjectMismatch,
    #[error("catalog drop object name cannot be empty")]
    EmptyObjectName,
    #[error("catalog drop metadata is incomplete for {kind:?}: {reason}")]
    InvalidMetadata { kind: CatalogKind, reason: String },
    #[error("catalog drop for {kind:?} is unsupported: {reason}")]
    Unsupported { kind: CatalogKind, reason: String },
}

impl CatalogDropRequest {
    pub fn new(connection: ConnectionIdentity, object: CatalogId, request_id: u64) -> Self {
        Self {
            connection,
            request_id,
            catalog_epoch: 0,
            object,
            entry: None,
            maintenance_database: None,
        }
    }

    pub fn with_entry(mut self, entry: CatalogEntry) -> Self {
        self.entry = Some(entry);
        self
    }

    pub fn with_maintenance_database(mut self, database: impl Into<String>) -> Self {
        self.maintenance_database = Some(database.into());
        self
    }

    pub fn validate(&self) -> Result<(), CatalogDropError> {
        validate_object_id(&self.object, self.connection.profile_id)
    }
}

/// A validated, execution-ready description of one catalog object drop.
///
/// SQL generation and execution are deliberately outside this model. Consumers
/// must retain the request identity and revalidate it at the execution boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogDropPlan {
    pub request: CatalogDropRequest,
    pub object: CatalogId,
    pub kind: CatalogKind,
    pub qualified_name: String,
    pub execution_target: CatalogDropExecutionTarget,
    behavior: CatalogDropBehavior,
    cascade_schema_name: Option<String>,
    sql: String,
}

impl CatalogDropPlan {
    pub fn new(
        request: CatalogDropRequest,
        entry: &CatalogEntry,
        sql: impl Into<String>,
    ) -> Result<Self, CatalogDropError> {
        request.validate()?;
        if entry.id != request.object {
            return Err(CatalogDropError::ObjectMismatch);
        }
        if entry.kind != entry.id.kind {
            return Err(CatalogDropError::KindMismatch {
                id_kind: entry.id.kind,
                entry_kind: entry.kind,
            });
        }
        if entry.qualified_name.object.is_empty() {
            return Err(CatalogDropError::EmptyObjectName);
        }
        let sql = sql.into();
        validate_sql(&sql)?;

        Ok(Self {
            request,
            object: entry.id.clone(),
            kind: entry.kind,
            qualified_name: entry.qualified_name.object.clone(),
            execution_target: CatalogDropExecutionTarget::CurrentConnection,
            behavior: CatalogDropBehavior::Restrict,
            cascade_schema_name: None,
            sql,
        })
    }

    pub(crate) fn postgres_schema_cascade(
        request: CatalogDropRequest,
        entry: &CatalogEntry,
        schema_name: &str,
    ) -> Result<Self, CatalogDropError> {
        if entry.kind != CatalogKind::Schema || entry.id.kind != CatalogKind::Schema {
            return Err(CatalogDropError::Unsupported {
                kind: entry.kind,
                reason: "CASCADE is only available for PostgreSQL schema drops".to_owned(),
            });
        }
        request.validate()?;
        if entry.id != request.object {
            return Err(CatalogDropError::ObjectMismatch);
        }
        let entry_schema = entry
            .qualified_name
            .schema
            .as_deref()
            .filter(|schema| !schema.is_empty())
            .ok_or(CatalogDropError::EmptyObjectName)?;
        if format!("\"{}\"", entry_schema.replace('"', "\"\"")) != schema_name {
            return Err(CatalogDropError::InvalidMetadata {
                kind: CatalogKind::Schema,
                reason: "schema name does not match the catalog entry".to_owned(),
            });
        }
        let sql = format!("DROP SCHEMA {schema_name} CASCADE");
        validate_sql_allow_schema_cascade(&sql)?;
        let Some(cascade_schema_name) = entry.qualified_name.schema.clone() else {
            return Err(CatalogDropError::EmptyObjectName);
        };
        let plan = Self {
            request,
            object: entry.id.clone(),
            kind: entry.kind,
            qualified_name: entry.qualified_name.object.clone(),
            execution_target: CatalogDropExecutionTarget::CurrentConnection,
            behavior: CatalogDropBehavior::Cascade,
            cascade_schema_name: Some(cascade_schema_name),
            sql,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn with_execution_target(mut self, target: CatalogDropExecutionTarget) -> Self {
        self.execution_target = target;
        self
    }

    pub fn sql(&self) -> &str {
        &self.sql
    }

    pub const fn behavior(&self) -> CatalogDropBehavior {
        self.behavior
    }

    pub fn with_request_id(mut self, request_id: u64) -> Result<Self, CatalogDropError> {
        self.request.request_id = request_id;
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), CatalogDropError> {
        self.request.validate()?;
        if self.object != self.request.object {
            return Err(CatalogDropError::ObjectMismatch);
        }
        if self.kind != self.object.kind {
            return Err(CatalogDropError::KindMismatch {
                id_kind: self.object.kind,
                entry_kind: self.kind,
            });
        }
        if self.qualified_name.is_empty() {
            return Err(CatalogDropError::EmptyObjectName);
        }
        match &self.execution_target {
            CatalogDropExecutionTarget::CurrentConnection if self.kind == CatalogKind::Database => {
                return Err(CatalogDropError::Unsupported {
                    kind: self.kind,
                    reason: "database drops require a maintenance database".to_owned(),
                });
            }
            CatalogDropExecutionTarget::MaintenanceDatabase(database)
                if self.kind != CatalogKind::Database || database.trim().is_empty() =>
            {
                return Err(CatalogDropError::Unsupported {
                    kind: self.kind,
                    reason: "maintenance execution is only valid for database drops".to_owned(),
                });
            }
            _ => {}
        }
        if let CatalogDropExecutionTarget::MaintenanceDatabase(database) = &self.execution_target
            && database.trim().is_empty()
        {
            return Err(CatalogDropError::EmptyObjectName);
        }
        if self.behavior == CatalogDropBehavior::Restrict {
            return validate_sql(&self.sql);
        }
        let Some(entry) = self.request.entry.as_ref() else {
            return Err(CatalogDropError::UnsafeSql);
        };
        let Some(schema_name) = self.cascade_schema_name.as_deref() else {
            return Err(CatalogDropError::UnsafeSql);
        };
        if self.kind != CatalogKind::Schema
            || entry.id != self.object
            || entry.kind != CatalogKind::Schema
            || self.qualified_name != schema_name
            || self.sql != format!("DROP SCHEMA {} CASCADE", quote_identifier(schema_name))
            || entry.qualified_name.schema.as_deref() != Some(schema_name)
            || entry.qualified_name.object != schema_name
            || entry.id.native_path.len() != 2
            || entry.id.native_path[1] != schema_name
            || entry
                .qualified_name
                .database
                .as_deref()
                .is_none_or(|database| entry.id.native_path[0] != database)
            || entry.parent_id.as_ref().is_none_or(|parent| {
                parent.connection_id != entry.id.connection_id
                    || parent.kind != CatalogKind::Database
                    || parent.native_path.as_slice() != [entry.id.native_path[0].as_str()]
            })
            || schema_name == "information_schema"
            || schema_name.starts_with("pg_")
            || self.execution_target != CatalogDropExecutionTarget::CurrentConnection
        {
            return Err(CatalogDropError::UnsafeSql);
        }
        validate_sql_allow_schema_cascade(&self.sql)
    }
}

fn validate_object_id(
    object: &CatalogId,
    connection_profile_id: Uuid,
) -> Result<(), CatalogDropError> {
    if object.native_path.is_empty() {
        return Err(CatalogDropError::EmptyObjectId);
    }
    if object.profile_id() != connection_profile_id {
        return Err(CatalogDropError::ProfileMismatch {
            object_profile_id: object.profile_id(),
            connection_profile_id,
        });
    }
    Ok(())
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn validate_sql(sql: &str) -> Result<(), CatalogDropError> {
    validate_sql_inner(sql, false)
}

fn validate_sql_allow_schema_cascade(sql: &str) -> Result<(), CatalogDropError> {
    validate_sql_inner(sql, true)
}

fn validate_sql_inner(sql: &str, allow_cascade: bool) -> Result<(), CatalogDropError> {
    let sql = sql.trim();
    if sql.is_empty() {
        return Err(CatalogDropError::EmptySql);
    }
    let mut token = String::new();
    let mut tokens = Vec::new();
    let mut quoted = None;
    let mut bracketed = false;
    let mut characters = sql.chars().peekable();
    while let Some(character) = characters.next() {
        if bracketed {
            if character == ']' && characters.next_if_eq(&']').is_none() {
                bracketed = false;
            }
            continue;
        }
        if let Some(delimiter) = quoted {
            if character == delimiter {
                quoted = None;
            }
            continue;
        }
        if matches!(character, '"' | '`') {
            quoted = Some(character);
        } else if character == '[' {
            bracketed = true;
        } else if character.is_ascii_alphanumeric() || character == '_' {
            token.push(character.to_ascii_uppercase());
        } else {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
            if character == ';' {
                return Err(CatalogDropError::UnsafeSql);
            }
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    if tokens.iter().any(|token| token == "CASCADE") && !allow_cascade
        || tokens.windows(2).any(|tokens| tokens == ["IF", "EXISTS"])
    {
        return Err(CatalogDropError::UnsafeSql);
    }
    Ok(())
}
