use std::fmt;

use mongodb::bson::{Bson, Document};
use serde::Deserialize;
use serde::de::{MapAccess, SeqAccess, Visitor};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MongoCommandRisk {
    ReadOnly,
    Write,
    Destructive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MongoCommandCapability {
    ReadDocuments,
    Count,
    Distinct,
    Aggregate,
    ListCollections,
    ListIndexes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MongoCommandKind {
    Find,
    Count,
    Distinct,
    Aggregate,
    Explain,
    ListCollections,
    ListIndexes,
    Ping,
    BuildInfo,
    DbStats,
    CollStats,
    ServerStatus,
    CurrentOp,
    Insert,
    Update,
    Delete,
    FindAndModify,
    Create,
    Drop,
    RenameCollection,
    CollMod,
    CreateIndexes,
    DropIndexes,
    UsersInfo,
    RolesInfo,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedMongoCommand {
    pub kind: MongoCommandKind,
    pub risk: MongoCommandRisk,
    pub database: String,
    pub namespaces: Vec<String>,
    pub command: Document,
    /// The first command key, retained independently from BSON map lookup.
    pub command_name: String,
}

impl ParsedMongoCommand {
    pub fn capability(&self) -> Result<MongoCommandCapability, MongoCommandError> {
        Ok(match self.kind {
            MongoCommandKind::Find => MongoCommandCapability::ReadDocuments,
            MongoCommandKind::Count => MongoCommandCapability::Count,
            MongoCommandKind::Distinct => MongoCommandCapability::Distinct,
            MongoCommandKind::Aggregate if self.risk == MongoCommandRisk::ReadOnly => {
                MongoCommandCapability::Aggregate
            }
            MongoCommandKind::ListCollections => MongoCommandCapability::ListCollections,
            MongoCommandKind::ListIndexes => MongoCommandCapability::ListIndexes,
            _ => {
                return Err(MongoCommandError::Unsupported(format!(
                    "{} is not in the read capability set",
                    self.command_name
                )));
            }
        })
    }

    pub fn validate_supported_fields(&self) -> Result<(), MongoCommandError> {
        if self.kind == MongoCommandKind::Explain {
            return Ok(());
        }
        let allowed: &[&str] = if self.risk != MongoCommandRisk::ReadOnly {
            match self.kind {
                MongoCommandKind::Aggregate => &[
                    "aggregate",
                    "pipeline",
                    "cursor",
                    "allowDiskUse",
                    "maxTimeMS",
                    "bypassDocumentValidation",
                    "comment",
                    "hint",
                    "collation",
                    "readConcern",
                ],
                MongoCommandKind::Insert => &[
                    "insert",
                    "documents",
                    "ordered",
                    "bypassDocumentValidation",
                    "comment",
                    "writeConcern",
                ],
                MongoCommandKind::Update => &[
                    "update",
                    "updates",
                    "ordered",
                    "bypassDocumentValidation",
                    "comment",
                    "writeConcern",
                ],
                MongoCommandKind::Delete => {
                    &["delete", "deletes", "ordered", "comment", "writeConcern"]
                }
                MongoCommandKind::FindAndModify => &[
                    "findAndModify",
                    "query",
                    "sort",
                    "remove",
                    "update",
                    "new",
                    "fields",
                    "upsert",
                    "bypassDocumentValidation",
                    "arrayFilters",
                    "collation",
                    "hint",
                    "let",
                    "comment",
                    "writeConcern",
                ],
                MongoCommandKind::Create => &[
                    "create",
                    "capped",
                    "size",
                    "max",
                    "timeseries",
                    "expireAfterSeconds",
                    "validator",
                    "validationLevel",
                    "validationAction",
                    "indexOptionDefaults",
                    "viewOn",
                    "pipeline",
                    "collation",
                    "writeConcern",
                    "comment",
                ],
                MongoCommandKind::Drop => &["drop", "writeConcern", "comment"],
                MongoCommandKind::RenameCollection => &[
                    "renameCollection",
                    "to",
                    "dropTarget",
                    "writeConcern",
                    "comment",
                ],
                MongoCommandKind::CollMod => &[
                    "collMod",
                    "validator",
                    "validationLevel",
                    "validationAction",
                    "viewOn",
                    "pipeline",
                    "index",
                    "keyPattern",
                    "hidden",
                    "expireAfterSeconds",
                    "writeConcern",
                    "comment",
                ],
                MongoCommandKind::CreateIndexes => &[
                    "createIndexes",
                    "indexes",
                    "commitQuorum",
                    "writeConcern",
                    "comment",
                ],
                MongoCommandKind::DropIndexes => &["dropIndexes", "writeConcern", "comment"],
                MongoCommandKind::Explain => unreachable!(),
                _ => return Err(MongoCommandError::Unsupported(self.command_name.clone())),
            }
        } else {
            match self.capability()? {
                MongoCommandCapability::ReadDocuments => &[
                    "find",
                    "filter",
                    "sort",
                    "projection",
                    "skip",
                    "limit",
                    "batchSize",
                    "maxTimeMS",
                    "hint",
                    "comment",
                    "collation",
                    "readConcern",
                ],
                MongoCommandCapability::Count => &[
                    "count",
                    "query",
                    "limit",
                    "skip",
                    "hint",
                    "maxTimeMS",
                    "comment",
                    "readConcern",
                ],
                MongoCommandCapability::Distinct => &[
                    "distinct",
                    "key",
                    "query",
                    "maxTimeMS",
                    "comment",
                    "readConcern",
                ],
                MongoCommandCapability::Aggregate => &[
                    "aggregate",
                    "pipeline",
                    "cursor",
                    "allowDiskUse",
                    "maxTimeMS",
                    "bypassDocumentValidation",
                    "comment",
                    "hint",
                    "collation",
                    "readConcern",
                ],
                MongoCommandCapability::ListCollections => &[
                    "listCollections",
                    "filter",
                    "nameOnly",
                    "authorizedCollections",
                    "cursor",
                    "comment",
                    "readConcern",
                ],
                MongoCommandCapability::ListIndexes => &[
                    "listIndexes",
                    "cursor",
                    "comment",
                    "maxTimeMS",
                    "readConcern",
                ],
            }
        };
        for (field, _) in self.command.iter() {
            if !allowed.contains(&field.as_str()) {
                return Err(MongoCommandError::Unsupported(format!(
                    "field `{field}` is not supported for {}",
                    self.command_name
                )));
            }
        }
        let require_string = |field: &str| -> Result<(), MongoCommandError> {
            if self.command.get_str(field).is_ok() {
                Ok(())
            } else {
                Err(MongoCommandError::Unsupported(format!(
                    "field `{field}` must be a string"
                )))
            }
        };
        match self.kind {
            MongoCommandKind::Find
            | MongoCommandKind::Count
            | MongoCommandKind::Distinct
            | MongoCommandKind::Aggregate
            | MongoCommandKind::ListIndexes => require_string(&self.command_name)?,
            MongoCommandKind::ListCollections => {}
            MongoCommandKind::Insert
            | MongoCommandKind::Update
            | MongoCommandKind::Delete
            | MongoCommandKind::Create
            | MongoCommandKind::Drop
            | MongoCommandKind::CreateIndexes
            | MongoCommandKind::DropIndexes
            | MongoCommandKind::CollMod => require_string(&self.command_name)?,
            MongoCommandKind::RenameCollection => require_string(&self.command_name)?,
            _ => {}
        }
        for field in ["skip", "limit", "batchSize", "maxTimeMS"] {
            if let Some(value) = self.command.get(field) {
                let valid = match value {
                    Bson::Int32(number) => *number >= 0,
                    Bson::Int64(number) => *number >= 0,
                    _ => false,
                };
                if !valid {
                    return Err(MongoCommandError::Unsupported(format!(
                        "field `{field}` must be a non-negative integer"
                    )));
                }
            }
        }
        if self.kind == MongoCommandKind::Aggregate && self.command.get_array("pipeline").is_err() {
            return Err(MongoCommandError::Unsupported(
                "field `pipeline` must be an array".into(),
            ));
        }
        if self.kind == MongoCommandKind::Distinct && self.command.get_str("key").is_err() {
            return Err(MongoCommandError::Unsupported(
                "field `key` must be a string".into(),
            ));
        }
        let array_fields: &[&str] = match self.kind {
            MongoCommandKind::Insert => &["documents"],
            MongoCommandKind::Update => &["updates"],
            MongoCommandKind::Delete => &["deletes"],
            MongoCommandKind::CreateIndexes => &["indexes"],
            _ => &[],
        };
        for field in array_fields {
            if self.command.get_array(field).is_err() {
                return Err(MongoCommandError::Unsupported(format!(
                    "field `{field}` must be an array"
                )));
            }
        }
        if self.kind == MongoCommandKind::Update {
            let updates = self.command.get_array("updates").map_err(|_| {
                MongoCommandError::Unsupported("field `updates` must be an array".into())
            })?;
            for update in updates {
                let update = update.as_document().ok_or_else(|| {
                    MongoCommandError::Unsupported("each update must be a document".into())
                })?;
                if update.keys().any(|key| {
                    !matches!(
                        key.as_str(),
                        "q" | "u" | "upsert" | "multi" | "collation" | "arrayFilters" | "hint"
                    )
                }) || update.get_document("q").is_err()
                    || update
                        .get("u")
                        .is_none_or(|value| !matches!(value, Bson::Document(_) | Bson::Array(_)))
                {
                    return Err(MongoCommandError::Unsupported(
                        "update entries must contain only q/u and supported options".into(),
                    ));
                }
            }
        }
        if self.kind == MongoCommandKind::Delete {
            let deletes = self.command.get_array("deletes").map_err(|_| {
                MongoCommandError::Unsupported("field `deletes` must be an array".into())
            })?;
            for delete in deletes {
                let delete = delete.as_document().ok_or_else(|| {
                    MongoCommandError::Unsupported("each delete must be a document".into())
                })?;
                if delete
                    .keys()
                    .any(|key| !matches!(key.as_str(), "q" | "limit" | "collation" | "hint"))
                    || delete.get_document("q").is_err()
                    || delete.get("limit").is_some_and(
                        |value| !matches!(value, Bson::Int32(limit) if (0..=2).contains(limit)),
                    )
                {
                    return Err(MongoCommandError::Unsupported(
                        "delete entries must contain a query and supported options".into(),
                    ));
                }
            }
        }
        for field in [
            "filter",
            "query",
            "sort",
            "projection",
            "hint",
            "readConcern",
        ] {
            if self
                .command
                .get(field)
                .is_some_and(|value| !matches!(value, Bson::Document(_)))
            {
                return Err(MongoCommandError::Unsupported(format!(
                    "field `{field}` must be a document"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum MongoCommandError {
    #[error("Mongo command must be a single JSON object")]
    InvalidRoot,
    #[error("Mongo command JSON is invalid: {0}")]
    Json(String),
    #[error("Mongo command contains duplicate object key `{0}`")]
    DuplicateKey(String),
    #[error("Mongo command must contain exactly one command key")]
    CommandKey,
    #[error("Mongo command `{0}` is unsupported")]
    Unsupported(String),
    #[error("Mongo command contains worker-managed field `{0}`")]
    ManagedField(String),
    #[error("Mongo command database is required")]
    DatabaseRequired,
    #[error("Mongo command contains unsupported executable expression `{0}`")]
    UnsupportedExpression(String),
}

#[derive(Debug)]
enum OrderedValue {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    String(String),
    Array(Vec<OrderedValue>),
    Object(Vec<(String, OrderedValue)>),
}

impl<'de> serde::Deserialize<'de> for OrderedValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OrderedVisitor;
        impl<'de> Visitor<'de> for OrderedVisitor {
            type Value = OrderedValue;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON value")
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(OrderedValue::Null)
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                self.visit_unit()
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(OrderedValue::Bool(v))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(OrderedValue::I64(v))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(OrderedValue::U64(v))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                Ok(OrderedValue::F64(v))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(OrderedValue::String(v.to_owned()))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(OrderedValue::String(v))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element()? {
                    values.push(value);
                }
                Ok(OrderedValue::Array(values))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, OrderedValue>()? {
                    if values
                        .iter()
                        .any(|(existing, _): &(String, OrderedValue)| existing == &key)
                    {
                        return Err(serde::de::Error::custom(format!("duplicate-key:{key}")));
                    }
                    values.push((key, value));
                }
                Ok(OrderedValue::Object(values))
            }
        }
        deserializer.deserialize_any(OrderedVisitor)
    }
}

pub fn parse_command(
    input: &str,
    selected_database: &str,
) -> Result<ParsedMongoCommand, MongoCommandError> {
    if input.len() > 1024 * 1024 {
        return Err(MongoCommandError::Json("command exceeds 1 MiB".into()));
    }
    let mut deserializer = serde_json::Deserializer::from_str(input);
    let ordered = OrderedValue::deserialize(&mut deserializer).map_err(|error| {
        let message = error.to_string();
        message
            .find("duplicate-key:")
            .map(|offset| {
                let key = message[offset + "duplicate-key:".len()..]
                    .split(['\n', ' '])
                    .next()
                    .unwrap_or_default();
                MongoCommandError::DuplicateKey(key.to_owned())
            })
            .unwrap_or(MongoCommandError::Json(message))
    })?;
    validate_complexity(&ordered)?;
    deserializer
        .end()
        .map_err(|error| MongoCommandError::Json(error.to_string()))?;
    let OrderedValue::Object(entries) = ordered else {
        return Err(MongoCommandError::InvalidRoot);
    };
    if entries.is_empty() {
        return Err(MongoCommandError::CommandKey);
    }
    let command_name = entries.first().expect("nonempty").0.clone();
    let kind = command_kind(&command_name)
        .ok_or_else(|| MongoCommandError::Unsupported(command_name.clone()))?;
    if entries.iter().skip(1).any(|(key, _)| is_command_name(key)) {
        return Err(MongoCommandError::CommandKey);
    }
    if let Some(field) = entries
        .iter()
        .find_map(|(key, value)| managed_field(key).or_else(|| find_managed_field(value)))
    {
        return Err(MongoCommandError::ManagedField(field));
    }
    if let Some(operator) = entries
        .iter()
        .find_map(|(_, value)| find_unsupported_expression(value))
    {
        return Err(MongoCommandError::UnsupportedExpression(operator));
    }
    let database = selected_database.to_owned();
    if database.is_empty() {
        return Err(MongoCommandError::DatabaseRequired);
    }
    let explain_inner = if kind == MongoCommandKind::Explain {
        let inner = entries
            .iter()
            .find(|(key, _)| key == "explain")
            .and_then(|(_, value)| match value {
                OrderedValue::Object(fields) => Some(fields),
                _ => None,
            })
            .ok_or_else(|| {
                MongoCommandError::Unsupported("explain must contain one command object".into())
            })?;
        if inner.is_empty() || inner.iter().skip(1).any(|(key, _)| is_command_name(key)) {
            return Err(MongoCommandError::CommandKey);
        }
        let inner_name = inner.first().expect("nonempty inner command").0.clone();
        let inner_kind = command_kind(&inner_name)
            .ok_or_else(|| MongoCommandError::Unsupported(inner_name.clone()))?;
        if inner_kind == MongoCommandKind::Explain {
            return Err(MongoCommandError::Unsupported(
                "nested explain commands are unsupported".into(),
            ));
        }
        let inner_risk = if matches!(
            inner_kind,
            MongoCommandKind::Drop
                | MongoCommandKind::RenameCollection
                | MongoCommandKind::DropIndexes
        ) {
            MongoCommandRisk::Destructive
        } else if is_write(inner_kind)
            || inner.iter().any(|(_, value)| contains_output_stage(value))
        {
            MongoCommandRisk::Write
        } else {
            MongoCommandRisk::ReadOnly
        };
        let parsed_inner = ParsedMongoCommand {
            kind: inner_kind,
            risk: inner_risk,
            database: selected_database.to_owned(),
            namespaces: collect_namespaces(
                inner_kind,
                &inner
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), clone_ordered(value)?)))
                    .collect::<Result<Vec<_>, MongoCommandError>>()?,
            ),
            command: ordered_document(
                inner
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), clone_ordered(value)?)))
                    .collect::<Result<_, MongoCommandError>>()?,
            )?,
            command_name: inner_name,
        };
        parsed_inner.validate_supported_fields()?;
        if parsed_inner.risk != MongoCommandRisk::ReadOnly {
            return Err(MongoCommandError::Unsupported(
                "explain cannot wrap a write or destructive command".into(),
            ));
        }
        Some(parsed_inner)
    } else {
        None
    };
    if kind == MongoCommandKind::Explain {
        for (key, _) in entries.iter().skip(1) {
            if !matches!(key.as_str(), "verbosity" | "comment") {
                return Err(MongoCommandError::Unsupported(format!(
                    "field `{key}` is not supported for explain"
                )));
            }
        }
        if entries.iter().find(|(key, _)| key == "verbosity").is_some_and(|(_, value)| {
            !matches!(value, OrderedValue::String(name) if matches!(name.as_str(), "queryPlanner" | "executionStats" | "allPlansExecution"))
        }) {
            return Err(MongoCommandError::Unsupported(
                "explain verbosity is invalid".into(),
            ));
        }
    }
    let risk = if let Some(inner) = &explain_inner {
        inner.risk
    } else if matches!(
        kind,
        MongoCommandKind::Drop | MongoCommandKind::RenameCollection | MongoCommandKind::DropIndexes
    ) {
        MongoCommandRisk::Destructive
    } else if is_write(kind)
        || entries
            .iter()
            .any(|(_, value)| contains_output_stage(value))
    {
        MongoCommandRisk::Write
    } else {
        MongoCommandRisk::ReadOnly
    };
    let namespaces = explain_inner
        .as_ref()
        .map(|inner| inner.namespaces.clone())
        .unwrap_or_else(|| collect_namespaces(kind, &entries));
    let command = ordered_document(entries)?;
    let parsed = ParsedMongoCommand {
        kind,
        risk,
        database,
        namespaces,
        command,
        command_name,
    };
    parsed.validate_supported_fields()?;
    Ok(parsed)
}

fn clone_ordered(value: &OrderedValue) -> Result<OrderedValue, MongoCommandError> {
    Ok(match value {
        OrderedValue::Null => OrderedValue::Null,
        OrderedValue::Bool(value) => OrderedValue::Bool(*value),
        OrderedValue::I64(value) => OrderedValue::I64(*value),
        OrderedValue::U64(value) => OrderedValue::U64(*value),
        OrderedValue::F64(value) => OrderedValue::F64(*value),
        OrderedValue::String(value) => OrderedValue::String(value.clone()),
        OrderedValue::Array(values) => {
            OrderedValue::Array(values.iter().map(clone_ordered).collect::<Result<_, _>>()?)
        }
        OrderedValue::Object(entries) => OrderedValue::Object(
            entries
                .iter()
                .map(|(key, value)| Ok((key.clone(), clone_ordered(value)?)))
                .collect::<Result<_, MongoCommandError>>()?,
        ),
    })
}

fn validate_complexity(value: &OrderedValue) -> Result<(), MongoCommandError> {
    const MAX_DEPTH: usize = 100;
    const MAX_NODES: usize = 100_000;

    fn visit(
        value: &OrderedValue,
        depth: usize,
        nodes: &mut usize,
    ) -> Result<(), MongoCommandError> {
        *nodes += 1;
        if *nodes > MAX_NODES {
            return Err(MongoCommandError::Json(
                "command exceeds the 100000 value complexity limit".into(),
            ));
        }
        if depth > MAX_DEPTH {
            return Err(MongoCommandError::Json(
                "command exceeds the 100-level nesting limit".into(),
            ));
        }
        match value {
            OrderedValue::Array(values) => {
                for child in values {
                    visit(child, depth + 1, nodes)?;
                }
            }
            OrderedValue::Object(entries) => {
                for (_, child) in entries {
                    visit(child, depth + 1, nodes)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    visit(value, 0, &mut 0)
}

fn ordered_document(entries: Vec<(String, OrderedValue)>) -> Result<Document, MongoCommandError> {
    let mut doc = Document::new();
    for (key, value) in entries {
        doc.insert(key, into_bson(value)?);
    }
    Ok(doc)
}
fn into_bson(value: OrderedValue) -> Result<Bson, MongoCommandError> {
    Ok(match value {
        OrderedValue::Null => Bson::Null,
        OrderedValue::Bool(v) => Bson::Boolean(v),
        OrderedValue::I64(v) => Bson::Int64(v),
        OrderedValue::U64(v) => i64::try_from(v)
            .map(Bson::Int64)
            .unwrap_or_else(|_| Bson::Double(v as f64)),
        OrderedValue::F64(v) => Bson::Double(v),
        OrderedValue::String(v) => Bson::String(v),
        OrderedValue::Array(values) => Bson::Array(
            values
                .into_iter()
                .map(into_bson)
                .collect::<Result<_, _>>()?,
        ),
        OrderedValue::Object(entries) => Bson::Document(ordered_document(entries)?),
    })
}
fn command_kind(name: &str) -> Option<MongoCommandKind> {
    Some(match name {
        "find" => MongoCommandKind::Find,
        "count" => MongoCommandKind::Count,
        "distinct" => MongoCommandKind::Distinct,
        "aggregate" => MongoCommandKind::Aggregate,
        "explain" => MongoCommandKind::Explain,
        "listCollections" => MongoCommandKind::ListCollections,
        "listIndexes" => MongoCommandKind::ListIndexes,
        "ping" => MongoCommandKind::Ping,
        "buildInfo" => MongoCommandKind::BuildInfo,
        "dbStats" => MongoCommandKind::DbStats,
        "collStats" => MongoCommandKind::CollStats,
        "serverStatus" => MongoCommandKind::ServerStatus,
        "currentOp" => MongoCommandKind::CurrentOp,
        "insert" => MongoCommandKind::Insert,
        "update" => MongoCommandKind::Update,
        "delete" => MongoCommandKind::Delete,
        "findAndModify" => MongoCommandKind::FindAndModify,
        "create" => MongoCommandKind::Create,
        "drop" => MongoCommandKind::Drop,
        "renameCollection" => MongoCommandKind::RenameCollection,
        "collMod" => MongoCommandKind::CollMod,
        "createIndexes" => MongoCommandKind::CreateIndexes,
        "dropIndexes" => MongoCommandKind::DropIndexes,
        "usersInfo" => MongoCommandKind::UsersInfo,
        "rolesInfo" => MongoCommandKind::RolesInfo,
        _ => return None,
    })
}
fn is_command_name(key: &str) -> bool {
    command_kind(key).is_some()
}
fn is_write(kind: MongoCommandKind) -> bool {
    matches!(
        kind,
        MongoCommandKind::Insert
            | MongoCommandKind::Update
            | MongoCommandKind::Delete
            | MongoCommandKind::FindAndModify
            | MongoCommandKind::Create
            | MongoCommandKind::Drop
            | MongoCommandKind::RenameCollection
            | MongoCommandKind::CollMod
            | MongoCommandKind::CreateIndexes
            | MongoCommandKind::DropIndexes
    )
}
fn contains_output_stage(value: &OrderedValue) -> bool {
    match value {
        OrderedValue::Object(entries) => entries.iter().any(|(key, value)| {
            if key == "$literal" {
                return false;
            }
            key == "$out" || key == "$merge" || contains_output_stage(value)
        }),
        OrderedValue::Array(values) => values.iter().any(contains_output_stage),
        _ => false,
    }
}

fn find_unsupported_expression(value: &OrderedValue) -> Option<String> {
    match value {
        OrderedValue::Object(entries) => entries.iter().find_map(|(key, value)| {
            if key == "$literal" {
                return None;
            }
            matches!(key.as_str(), "$where" | "$function" | "$accumulator")
                .then(|| key.clone())
                .or_else(|| find_unsupported_expression(value))
        }),
        OrderedValue::Array(values) => values.iter().find_map(find_unsupported_expression),
        _ => None,
    }
}

fn managed_field(key: &str) -> Option<String> {
    matches!(
        key,
        "$db"
            | "lsid"
            | "txnNumber"
            | "autocommit"
            | "startTransaction"
            | "getMore"
            | "killCursors"
    )
    .then(|| key.to_owned())
}

fn find_managed_field(value: &OrderedValue) -> Option<String> {
    match value {
        OrderedValue::Object(entries) => entries.iter().find_map(|(key, value)| {
            if key == "$literal" {
                return None;
            }
            managed_field(key).or_else(|| find_managed_field(value))
        }),
        OrderedValue::Array(values) => values.iter().find_map(find_managed_field),
        _ => None,
    }
}
fn collect_namespaces(kind: MongoCommandKind, entries: &[(String, OrderedValue)]) -> Vec<String> {
    let mut names = Vec::new();
    if matches!(
        kind,
        MongoCommandKind::Find
            | MongoCommandKind::Count
            | MongoCommandKind::Distinct
            | MongoCommandKind::Aggregate
            | MongoCommandKind::ListIndexes
            | MongoCommandKind::Insert
            | MongoCommandKind::Update
            | MongoCommandKind::Delete
            | MongoCommandKind::FindAndModify
            | MongoCommandKind::Create
            | MongoCommandKind::Drop
            | MongoCommandKind::CollMod
            | MongoCommandKind::CreateIndexes
            | MongoCommandKind::DropIndexes
    ) && let Some((_, OrderedValue::String(collection))) = entries.first()
    {
        names.push(collection.clone());
    }
    if kind == MongoCommandKind::RenameCollection {
        if let Some((_, OrderedValue::String(from))) = entries.first() {
            names.push(from.clone());
        }
        if let Some((_, OrderedValue::String(to))) = entries.iter().find(|(key, _)| key == "to") {
            names.push(to.clone());
        }
    }
    for (key, value) in entries.iter().skip(1) {
        if key == "pipeline" {
            collect_output_names(value, &mut names);
        }
        if kind == MongoCommandKind::RenameCollection && key == "to" {
            collect_namespace_value(value, &mut names);
        }
    }
    names.sort();
    names.dedup();
    names
}
fn collect_output_names(value: &OrderedValue, names: &mut Vec<String>) {
    match value {
        OrderedValue::Object(entries) => {
            for (key, value) in entries {
                if key == "$out" || key == "$merge" {
                    match value {
                        OrderedValue::String(namespace) => names.push(namespace.clone()),
                        OrderedValue::Object(fields) => {
                            let db = fields.iter().find_map(|(k, v)| {
                                if k == "db" {
                                    if let OrderedValue::String(s) = v {
                                        Some(s.as_str())
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            });
                            let coll = fields.iter().find_map(|(k, v)| {
                                if k == "coll" {
                                    if let OrderedValue::String(s) = v {
                                        Some(s.as_str())
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            });
                            if let (Some(db), Some(coll)) = (db, coll) {
                                names.push(format!("{db}.{coll}"));
                            }
                        }
                        _ => {}
                    }
                }
                if key == "$lookup" || key == "$unionWith" {
                    collect_reference_name(value, names);
                    continue;
                }
                if key == "$literal" {
                    continue;
                }
                collect_output_names(value, names);
            }
        }
        OrderedValue::Array(values) => {
            for value in values {
                collect_output_names(value, names);
            }
        }
        _ => {}
    }
}

fn collect_namespace_value(value: &OrderedValue, names: &mut Vec<String>) {
    if let OrderedValue::String(name) = value {
        names.push(name.clone());
    }
}

fn collect_reference_name(value: &OrderedValue, names: &mut Vec<String>) {
    match value {
        OrderedValue::String(name) => names.push(name.clone()),
        OrderedValue::Object(fields) => {
            if let Some((_, OrderedValue::Object(from_fields))) =
                fields.iter().find(|(key, _)| key == "from")
            {
                let db = from_fields.iter().find_map(|(key, value)| {
                    if key == "db" {
                        if let OrderedValue::String(name) = value {
                            Some(name.as_str())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                });
                let coll = from_fields.iter().find_map(|(key, value)| {
                    if key == "coll" {
                        if let OrderedValue::String(name) = value {
                            Some(name.as_str())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                });
                if let (Some(db), Some(coll)) = (db, coll) {
                    names.push(format!("{db}.{coll}"));
                    return;
                }
            }
            let db = fields.iter().find_map(|(key, value)| {
                if key == "db" {
                    if let OrderedValue::String(name) = value {
                        Some(name.as_str())
                    } else {
                        None
                    }
                } else {
                    None
                }
            });
            let coll = fields.iter().find_map(|(key, value)| {
                if key == "coll" || key == "from" {
                    if let OrderedValue::String(name) = value {
                        Some(name.as_str())
                    } else {
                        None
                    }
                } else {
                    None
                }
            });
            if let (Some(db), Some(coll)) = (db, coll) {
                names.push(format!("{db}.{coll}"));
            } else if let Some(coll) = coll {
                names.push(coll.to_owned());
            }
        }
        _ => {}
    }
}

impl fmt::Display for MongoCommandKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
