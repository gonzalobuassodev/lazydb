use uuid::Uuid;

use crate::mongodb_command::{MongoCommandRisk, ParsedMongoCommand, parse_command};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MongoConsoleStatus {
    Idle,
    Running { generation: u64 },
    Succeeded,
    Failed(String),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MongoConsoleTransactionState {
    #[default]
    Idle,
    Starting,
    Active,
    Committing,
    Aborting,
    Committed,
    Aborted,
    OutcomeUnknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MongoTransactionControl {
    Begin,
    Commit,
    Abort,
}

pub fn parse_transaction_control(input: &str) -> Result<Option<MongoTransactionControl>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|error| error.to_string())?;
    let object = value
        .as_object()
        .ok_or_else(|| "Mongo transaction input must be a JSON object".to_owned())?;
    if !object.contains_key("$transaction") {
        return Ok(None);
    }
    if object.len() != 1 {
        return Err("Mongo transaction input must contain exactly one field".into());
    }
    match object.keys().next().map(String::as_str) {
        Some("$transaction") => match object.values().next().and_then(serde_json::Value::as_str) {
            Some("begin") => Ok(Some(MongoTransactionControl::Begin)),
            Some("commit") => Ok(Some(MongoTransactionControl::Commit)),
            Some("abort") => Ok(Some(MongoTransactionControl::Abort)),
            _ => Err("$transaction must be begin, commit, or abort".into()),
        },
        _ => Ok(None),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MongoConsoleResult {
    pub documents_ejson: Vec<String>,
    pub reply_ejson: Option<String>,
    pub count: Option<u64>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MongoConsoleTab {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub database: String,
    pub title: String,
    pub command_text: String,
    pub status: MongoConsoleStatus,
    pub result: Option<MongoConsoleResult>,
    pub generation: u64,
    pub transaction_generation: u64,
    pub transaction: MongoConsoleTransactionState,
    pub transaction_connection_generation: Option<u64>,
}

impl MongoConsoleTab {
    pub fn new(profile_id: Uuid, database: impl Into<String>) -> Self {
        let database = database.into();
        Self {
            id: Uuid::new_v4(),
            profile_id,
            title: format!("Mongo · {database}"),
            database,
            command_text: "{\n  \"find\": \"collection\",\n  \"filter\": {}\n}".into(),
            status: MongoConsoleStatus::Idle,
            result: None,
            generation: 0,
            transaction_generation: 0,
            transaction: MongoConsoleTransactionState::Idle,
            transaction_connection_generation: None,
        }
    }

    pub fn validate_read_command(&self) -> Result<ParsedMongoCommand, String> {
        if parse_transaction_control(&self.command_text)?.is_some() {
            return Err("transaction control must be dispatched as a transaction action".into());
        }
        let parsed =
            parse_command(&self.command_text, &self.database).map_err(|error| error.to_string())?;
        if parsed.risk != MongoCommandRisk::ReadOnly {
            return Err("Mongo console currently runs read-only commands".into());
        }
        if let Some(limit) = parsed.command.get_i64("limit").ok()
            && !(1..=500).contains(&limit)
        {
            return Err("Mongo console limit must be between 1 and 500".into());
        }
        parsed.capability().map_err(|error| error.to_string())?;
        parsed
            .validate_supported_fields()
            .map_err(|error| error.to_string())?;
        Ok(parsed)
    }

    pub fn explain_command(&self) -> Result<ParsedMongoCommand, String> {
        let inner = self.validate_read_command()?;
        if matches!(
            inner.kind,
            crate::mongodb_command::MongoCommandKind::Find
                | crate::mongodb_command::MongoCommandKind::Count
                | crate::mongodb_command::MongoCommandKind::Distinct
                | crate::mongodb_command::MongoCommandKind::Aggregate
        ) {
            Ok(inner)
        } else {
            Err("Explain is available for find, count, distinct, and aggregate".into())
        }
    }

    pub fn begin(&mut self) -> Result<u64, String> {
        self.validate_read_command()?;
        self.generation = self.generation.wrapping_add(1);
        self.status = MongoConsoleStatus::Running {
            generation: self.generation,
        };
        self.result = None;
        Ok(self.generation)
    }

    pub fn complete(&mut self, generation: u64, result: MongoConsoleResult) -> bool {
        if self.generation != generation {
            return false;
        }
        self.result = Some(result);
        self.status = MongoConsoleStatus::Succeeded;
        true
    }

    pub fn fail(&mut self, generation: u64, message: String) -> bool {
        if self.generation != generation {
            return false;
        }
        self.result = None;
        self.status = MongoConsoleStatus::Failed(message);
        true
    }

    pub fn set_command(&mut self, text: String) {
        self.command_text = text;
        self.result = None;
        self.status = MongoConsoleStatus::Idle;
    }

    pub fn begin_with_command(&mut self, command: String) -> Result<u64, String> {
        self.command_text = command;
        self.begin()
    }

    pub fn begin_transaction(&mut self) -> Result<u64, String> {
        if self.transaction != MongoConsoleTransactionState::Idle {
            return Err("MongoDB transaction is already active or unresolved".into());
        }
        self.transaction_generation = self.transaction_generation.wrapping_add(1);
        self.transaction = MongoConsoleTransactionState::Starting;
        Ok(self.transaction_generation)
    }

    pub fn begin_transaction_command(&mut self, command: String) -> Result<u64, String> {
        if command.trim().is_empty() {
            return self.begin_transaction();
        }
        let parsed = parse_command(&command, &self.database).map_err(|error| error.to_string())?;
        if parsed.kind != crate::mongodb_command::MongoCommandKind::Find {
            return Err("Start a transaction from a collection find command; use the transaction command action for writes".into());
        }
        if parsed.risk == MongoCommandRisk::ReadOnly {
            parsed
                .validate_supported_fields()
                .map_err(|error| error.to_string())?;
        }
        let generation = self.begin_transaction()?;
        Ok(generation)
    }

    pub fn set_transaction_result(
        &mut self,
        generation: u64,
        state: MongoConsoleTransactionState,
    ) -> bool {
        if self.transaction_generation != generation {
            return false;
        }
        self.transaction = state;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_write_and_unsupported_command_without_mutating_running_state() {
        let mut tab = MongoConsoleTab::new(Uuid::new_v4(), "app");
        tab.set_command(r#"{"insert":"items","documents":[{"x":1}]}"#.into());
        assert!(tab.begin().is_err());
        assert_eq!(tab.status, MongoConsoleStatus::Idle);
    }

    #[test]
    fn explain_selection_is_limited_to_supported_bounded_read_commands() {
        let mut tab = MongoConsoleTab::new(Uuid::new_v4(), "app");
        for command in [
            r#"{"find":"items","filter":{},"limit":20}"#,
            r#"{"count":"items","query":{}}"#,
            r#"{"distinct":"items","key":"name"}"#,
            r#"{"aggregate":"items","pipeline":[{"$match":{}}]}"#,
        ] {
            tab.set_command(command.into());
            assert!(tab.explain_command().is_ok(), "{command}");
        }
        for command in [
            r#"{"listCollections":1}"#,
            r#"{"find":"items","limit":501}"#,
            r#"{"drop":"items"}"#,
        ] {
            tab.set_command(command.into());
            assert!(tab.explain_command().is_err(), "{command}");
        }
    }

    #[test]
    fn ordinary_command_objects_are_not_transaction_controls() {
        assert_eq!(
            parse_transaction_control(r#"{"find":"items","filter":{}}"#).unwrap(),
            None
        );
    }

    #[test]
    fn stale_completion_cannot_replace_newer_execution() {
        let mut tab = MongoConsoleTab::new(Uuid::new_v4(), "app");
        let old = tab.begin().unwrap();
        let new = tab.begin().unwrap();
        let result = MongoConsoleResult {
            documents_ejson: vec![],
            reply_ejson: None,
            count: Some(3),
            truncated: false,
        };
        assert!(!tab.complete(old, result.clone()));
        assert!(tab.complete(new, result));
        assert_eq!(tab.status, MongoConsoleStatus::Succeeded);
    }

    #[test]
    fn transaction_generation_rejects_stale_results_and_keeps_unknown_explicit() {
        let mut tab = MongoConsoleTab::new(Uuid::new_v4(), "app");
        let first = tab.begin_transaction().unwrap();
        tab.set_transaction_result(first, MongoConsoleTransactionState::Active);
        tab.transaction = MongoConsoleTransactionState::Idle;
        let second = tab.begin_transaction().unwrap();
        assert!(!tab.set_transaction_result(first, MongoConsoleTransactionState::Committed));
        assert!(tab.set_transaction_result(second, MongoConsoleTransactionState::OutcomeUnknown));
        assert_eq!(
            tab.begin_transaction(),
            Err("MongoDB transaction is already active or unresolved".into())
        );
    }

    #[test]
    fn transaction_control_parser_accepts_only_explicit_single_field_forms() {
        assert_eq!(
            parse_transaction_control(r#"{"$transaction":"begin"}"#).unwrap(),
            Some(MongoTransactionControl::Begin)
        );
        assert_eq!(
            parse_transaction_control(r#"{"$transaction":"commit"}"#).unwrap(),
            Some(MongoTransactionControl::Commit)
        );
        assert!(parse_transaction_control(r#"{"$transaction":"drop"}"#).is_err());
        assert!(parse_transaction_control(r#"{"$transaction":"abort","$db":"admin"}"#).is_err());
    }

    #[test]
    fn transaction_begin_requires_a_supported_collection_find() {
        let mut tab = MongoConsoleTab::new(Uuid::new_v4(), "app");
        assert!(
            tab.begin_transaction_command(r#"{"insert":"items","documents":[]}"#.into())
                .is_err()
        );
        assert!(
            tab.begin_transaction_command(r#"{"find":"items","$db":"admin"}"#.into())
                .is_err()
        );
        assert!(
            tab.begin_transaction_command(r#"{"find":"items","filter":{}}"#.into())
                .is_ok()
        );
    }
}
