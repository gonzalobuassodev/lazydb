use uuid::Uuid;

use super::{sql_history::ExecutionHistory, text_input::TextInput};

/// Which part of the SQL History overlay currently owns keyboard input.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SqlHistoryMode {
    #[default]
    Browse,
    Search,
    Sql,
}

/// Loading identity for a history request.
///
/// The overlay instance is intentionally part of the identity. A response
/// from a closed overlay must not be allowed to update a newly opened one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqlHistoryRequest {
    pub overlay_id: Uuid,
    pub generation: u64,
    pub cursor: Option<crate::persistence::sql_history::HistoryCursor>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqlHistoryState {
    pub overlay_id: Uuid,
    pub mode: SqlHistoryMode,
    pub selected_execution: Option<Uuid>,
    pub list_offset: usize,
    pub search: TextInput,
    pub status_filter: Option<crate::model::sql_history::HistoryExecutionStatus>,
    pub transaction_filter: Option<crate::model::sql_history::HistoryTransactionOutcome>,
    pub database_filter: Option<String>,
    pub query_generation: u64,
    pub items: Vec<ExecutionHistory>,
    pub loading: bool,
    pub next_cursor: Option<crate::persistence::sql_history::HistoryCursor>,
    pub in_flight: Option<SqlHistoryRequest>,
    pub error: Option<String>,
    pub editor_session_id: Uuid,
    pub loaded_execution_id: Option<Uuid>,
    pub sql_offset: usize,
    pub mongo_detail_text: Option<String>,
}

impl Default for SqlHistoryState {
    fn default() -> Self {
        Self::new()
    }
}

impl SqlHistoryState {
    pub fn new() -> Self {
        Self {
            overlay_id: Uuid::new_v4(),
            mode: SqlHistoryMode::Browse,
            selected_execution: None,
            list_offset: 0,
            search: TextInput::default(),
            status_filter: None,
            transaction_filter: None,
            database_filter: None,
            query_generation: 0,
            items: Vec::new(),
            loading: false,
            next_cursor: None,
            in_flight: None,
            error: None,
            editor_session_id: Uuid::new_v4(),
            loaded_execution_id: None,
            sql_offset: 0,
            mongo_detail_text: None,
        }
    }

    pub fn selected_item(&self) -> Option<&ExecutionHistory> {
        let id = self.selected_execution?;
        self.items.iter().find(|item| item.execution_id == id)
    }

    pub fn visible_ids(&self) -> impl Iterator<Item = Uuid> + '_ {
        self.items.iter().map(|item| item.execution_id)
    }

    pub fn select_index(&mut self, index: usize) -> bool {
        let Some(item) = self.items.get(index) else {
            return false;
        };
        let changed = self.selected_execution != Some(item.execution_id);
        self.selected_execution = Some(item.execution_id);
        if changed {
            self.sql_offset = 0;
            self.loaded_execution_id = None;
            self.mongo_detail_text = None;
            self.mongo_detail_text = None;
        }
        true
    }

    pub fn move_selection(&mut self, delta: isize) -> bool {
        if self.items.is_empty() {
            self.selected_execution = None;
            return false;
        }
        let current = self
            .selected_execution
            .and_then(|id| self.items.iter().position(|item| item.execution_id == id))
            .unwrap_or(0);
        let next = if delta == isize::MIN {
            0
        } else if delta == isize::MAX {
            self.items.len() - 1
        } else {
            (current as isize + delta).rem_euclid(self.items.len() as isize) as usize
        };
        self.select_index(next)
    }

    pub fn reconcile_selection(&mut self) {
        if self
            .selected_execution
            .is_some_and(|id| self.items.iter().any(|item| item.execution_id == id))
        {
            return;
        }
        self.selected_execution = self.items.first().map(|item| item.execution_id);
        self.list_offset = 0;
        self.sql_offset = 0;
        self.loaded_execution_id = None;
    }

    pub fn begin_query(&mut self) -> u64 {
        self.query_generation = self.query_generation.saturating_add(1);
        self.loading = true;
        self.error = None;
        self.next_cursor = None;
        self.in_flight = None;
        self.items.clear();
        self.selected_execution = None;
        self.list_offset = 0;
        self.sql_offset = 0;
        self.loaded_execution_id = None;
        self.query_generation
    }

    pub fn request(
        &mut self,
        cursor: Option<crate::persistence::sql_history::HistoryCursor>,
    ) -> SqlHistoryRequest {
        let request = SqlHistoryRequest {
            overlay_id: self.overlay_id,
            generation: self.query_generation,
            cursor,
        };
        self.loading = true;
        self.in_flight = Some(request.clone());
        request
    }

    pub fn accepts(&self, request: &SqlHistoryRequest) -> bool {
        self.overlay_id == request.overlay_id
            && self.query_generation == request.generation
            && self.in_flight.as_ref() == Some(request)
    }

    pub fn complete(
        &mut self,
        request: &SqlHistoryRequest,
        items: Vec<ExecutionHistory>,
        next_cursor: Option<crate::persistence::sql_history::HistoryCursor>,
    ) -> bool {
        if !self.accepts(request) {
            return false;
        }
        let append = request.cursor.is_some();
        if append {
            for item in items {
                if !self
                    .items
                    .iter()
                    .any(|existing| existing.execution_id == item.execution_id)
                {
                    self.items.push(item);
                }
            }
        } else {
            self.items = items;
        }
        self.next_cursor = next_cursor;
        self.loading = false;
        self.error = None;
        self.in_flight = None;
        self.reconcile_selection();
        true
    }

    pub fn fail(&mut self, request: &SqlHistoryRequest, message: impl Into<String>) -> bool {
        if !self.accepts(request) {
            return false;
        }
        self.loading = false;
        self.in_flight = None;
        self.error = Some(message.into());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::sql_history::{
        HistoryExecutionStatus, HistoryResultCertainty, HistoryTransactionOutcome,
    };

    fn item(id: Uuid, sql: &str) -> ExecutionHistory {
        ExecutionHistory {
            execution_id: id,
            operation_id: Uuid::new_v4(),
            transaction_id: None,
            sql: sql.into(),
            status: HistoryExecutionStatus::Succeeded,
            certainty: HistoryResultCertainty::Confirmed,
            transaction_outcome: HistoryTransactionOutcome::AutoCommitted,
            affected_rows: None,
            returned_rows: None,
            requested_at: 0,
            elapsed_millis: None,
            profile_id: None,
            database: None,
            schema: None,
            language: crate::model::sql_history::HistoryLanguage::Sql,
            mongo_command: None,
        }
    }

    #[test]
    fn selection_is_reconciled_and_detail_offset_resets() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let mut state = SqlHistoryState::new();
        state.items = vec![item(first, "select 1"), item(second, "select 2")];

        assert!(state.select_index(1));
        state.sql_offset = 7;
        assert!(state.move_selection(-1));
        assert_eq!(state.selected_execution, Some(first));
        assert_eq!(state.sql_offset, 0);
        state.items.clear();
        state.reconcile_selection();
        assert_eq!(state.selected_execution, None);
    }

    #[test]
    fn stale_request_cannot_replace_new_query() {
        let id = Uuid::new_v4();
        let mut state = SqlHistoryState::new();
        state.begin_query();
        let old = state.request(None);
        state.begin_query();
        let current = state.request(None);

        assert!(!state.complete(&old, vec![item(id, "old")], None));
        assert!(state.items.is_empty());
        assert!(state.complete(&current, vec![item(id, "current")], None));
        assert_eq!(
            state.selected_item().map(|item| item.sql.as_str()),
            Some("current")
        );
    }

    #[test]
    fn append_deduplicates_and_preserves_selection() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let mut state = SqlHistoryState::new();
        state.begin_query();
        let initial = state.request(None);
        state.complete(
            &initial,
            vec![item(first, "one")],
            Some(crate::persistence::sql_history::HistoryCursor {
                requested_at: 1,
                execution_id: first,
            }),
        );
        state.select_index(0);
        let page = state.request(state.next_cursor.clone());
        assert!(state.complete(&page, vec![item(first, "one"), item(second, "two")], None));
        assert_eq!(state.items.len(), 2);
        assert_eq!(state.selected_execution, Some(first));
    }
}
