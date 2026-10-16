use uuid::Uuid;

use crate::db::mongodb::types::{FindSpec, MongoDocumentPage, MongoNamespace};

#[derive(Clone, Debug, PartialEq)]
pub enum MongoPageState {
    Empty,
    Loading,
    Ready(MongoDocumentPage),
    Failed(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MongoQueryField {
    Filter,
    Sort,
    Projection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MongoImportDraft {
    pub documents: Vec<String>,
    pub accepted: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MongoMutationKind {
    Insert,
    Delete,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MongoBrowserTab {
    pub id: Uuid,
    pub title: String,
    pub profile_id: Uuid,
    pub namespace: MongoNamespace,
    pub spec: FindSpec,
    pub page: MongoPageState,
    pub selected_document: usize,
    pub detail_editor_id: Uuid,
    pub generation: u64,
    pub connection_generation: Option<u64>,
    pub editing: bool,
    pub edit_text: String,
    pub edit_error: Option<String>,
    pub query_field: Option<MongoQueryField>,
    pub query_text: String,
    pub query_error: Option<String>,
    pub import_draft: Option<MongoImportDraft>,
    pub mutation_message: Option<String>,
    pub mutation_generation: u64,
}

impl MongoBrowserTab {
    pub fn new(profile_id: Uuid, namespace: MongoNamespace) -> Self {
        let title = namespace
            .collection
            .clone()
            .unwrap_or_else(|| namespace.database.clone());
        Self {
            id: Uuid::new_v4(),
            title,
            profile_id,
            spec: FindSpec::new(namespace.clone()),
            namespace,
            page: MongoPageState::Empty,
            selected_document: 0,
            detail_editor_id: Uuid::new_v4(),
            generation: 0,
            connection_generation: None,
            editing: false,
            edit_text: String::new(),
            edit_error: None,
            query_field: None,
            query_text: String::new(),
            query_error: None,
            import_draft: None,
            mutation_message: None,
            mutation_generation: 0,
        }
    }

    pub fn begin_load(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.page = MongoPageState::Loading;
        self.generation
    }

    pub fn begin_mutation(&mut self) -> u64 {
        self.mutation_generation = self.mutation_generation.wrapping_add(1);
        self.mutation_message = Some("Mutation pending".into());
        self.mutation_generation
    }

    pub fn accept_mutation_outcome(
        &mut self,
        generation: u64,
        outcome: &crate::db::mongodb::mutation::MongoMutationOutcome,
    ) -> bool {
        if generation != self.mutation_generation {
            return false;
        }
        self.mutation_message = Some(match outcome {
            crate::db::mongodb::mutation::MongoMutationOutcome::Confirmed { affected } => {
                format!("Mutation confirmed: {affected} document(s)")
            }
            crate::db::mongodb::mutation::MongoMutationOutcome::Partial {
                affected,
                requested,
            } => format!(
                "Partial result: {affected} of {requested} documents inserted; review the collection before retrying"
            ),
            crate::db::mongodb::mutation::MongoMutationOutcome::Unknown { requested } => format!(
                "Outcome unknown for {requested} operation(s); inspect the collection before retrying"
            ),
        });
        true
    }

    pub fn accept_page(&mut self, generation: u64, page: MongoDocumentPage) -> bool {
        if generation != self.generation {
            return false;
        }
        self.selected_document = self
            .selected_document
            .min(page.documents.len().saturating_sub(1));
        self.page = MongoPageState::Ready(page);
        true
    }

    pub fn selected_document_ejson(&self) -> Option<String> {
        let MongoPageState::Ready(page) = &self.page else {
            return None;
        };
        page.documents
            .get(self.selected_document)?
            .canonical_ejson()
            .ok()
    }

    pub fn selected_document_inspector(&self) -> Option<(String, bool)> {
        let MongoPageState::Ready(page) = &self.page else {
            return None;
        };
        let document = page.documents.get(self.selected_document)?;
        if document.editable {
            Some((document.canonical_ejson().ok()?, false))
        } else {
            let hex = document
                .raw
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" ");
            Some((
                format!(
                    "Raw BSON · {} bytes\n{}",
                    document.raw.as_bytes().len(),
                    hex
                ),
                true,
            ))
        }
    }

    /// Export only the page currently held in memory, with explicit partial
    /// state so it cannot be mistaken for a collection backup.
    pub fn loaded_page_ejson(&self) -> Result<(String, usize, bool), String> {
        let MongoPageState::Ready(page) = &self.page else {
            return Err("no MongoDB document page is loaded".to_owned());
        };
        if page.documents.iter().any(|document| !document.editable) {
            return Err(
                "page contains raw-only BSON documents that cannot be represented as EJSON"
                    .to_owned(),
            );
        }
        let values = page
            .documents
            .iter()
            .map(|document| {
                let bson =
                    mongodb::bson::to_bson(&document.bson).map_err(|error| error.to_string())?;
                Ok(bson.into_canonical_extjson())
            })
            .collect::<Result<Vec<_>, String>>()?;
        let ejson = serde_json::to_string_pretty(&values).map_err(|error| error.to_string())?;
        Ok((
            ejson,
            page.documents.len(),
            page.next_offset.is_some() || page.truncated,
        ))
    }

    pub fn begin_edit(&mut self) -> bool {
        let MongoPageState::Ready(page) = &self.page else {
            return false;
        };
        if !page
            .documents
            .get(self.selected_document)
            .is_some_and(|document| document.editable)
        {
            return false;
        }
        let Some(text) = self.selected_document_ejson() else {
            return false;
        };
        self.edit_text = text;
        self.edit_error = None;
        self.editing = true;
        true
    }

    pub fn cancel_edit(&mut self) {
        self.editing = false;
        self.edit_error = None;
    }

    pub fn edited_document(&self) -> Result<mongodb::bson::Document, String> {
        let value: serde_json::Value =
            serde_json::from_str(&self.edit_text).map_err(|error| error.to_string())?;
        let bson = mongodb::bson::Bson::try_from(value).map_err(|error| error.to_string())?;
        match bson {
            mongodb::bson::Bson::Document(document) => Ok(document),
            other => Err(format!(
                "expected a BSON document, got {:?}",
                other.element_type()
            )),
        }
    }

    pub fn edited_document_preserves_id(&self) -> Result<mongodb::bson::Document, String> {
        let replacement = self.edited_document()?;
        let MongoPageState::Ready(page) = &self.page else {
            return Err("document page is no longer available".into());
        };
        let original = page
            .documents
            .get(self.selected_document)
            .and_then(|document| document.bson.get("_id"));
        if original != replacement.get("_id") {
            return Err("_id cannot be changed during replacement".into());
        }
        Ok(replacement)
    }

    pub fn begin_query_edit(&mut self, field: MongoQueryField) {
        let current = match field {
            MongoQueryField::Filter => Some(self.spec.filter.clone()),
            MongoQueryField::Sort => Some(self.spec.sort.clone().unwrap_or_default()),
            MongoQueryField::Projection => Some(self.spec.projection.clone().unwrap_or_default()),
        };
        self.query_text = current
            .map(|document| {
                mongodb::bson::to_bson(&document)
                    .unwrap_or_default()
                    .into_relaxed_extjson()
                    .to_string()
            })
            .unwrap_or_else(|| "{}".to_owned());
        self.query_error = None;
        self.query_field = Some(field);
    }

    pub fn submit_query_edit(&mut self) -> Result<(), String> {
        let field = self
            .query_field
            .ok_or_else(|| "no query field is being edited".to_owned())?;
        let json: serde_json::Value =
            serde_json::from_str(&self.query_text).map_err(|error| error.to_string())?;
        let bson = mongodb::bson::Bson::try_from(json).map_err(|error| error.to_string())?;
        let mongodb::bson::Bson::Document(document) = bson else {
            return Err("query value must be a JSON object".into());
        };
        match field {
            MongoQueryField::Filter => self.spec.filter = document,
            MongoQueryField::Sort => self.spec.sort = (!document.is_empty()).then_some(document),
            MongoQueryField::Projection => {
                self.spec.projection = (!document.is_empty()).then_some(document)
            }
        }
        self.query_field = None;
        self.query_error = None;
        self.spec.skip = 0;
        Ok(())
    }

    pub fn set_import_draft(&mut self, input: &str) -> Result<(), String> {
        let value: serde_json::Value =
            serde_json::from_str(input).map_err(|error| error.to_string())?;
        let values = match value {
            serde_json::Value::Array(values) => values,
            value => vec![value],
        };
        if values.is_empty() || values.len() > 500 {
            return Err("import must contain 1..=500 documents".into());
        }
        let mut documents = Vec::with_capacity(values.len());
        for value in values {
            let bson = mongodb::bson::Bson::try_from(value).map_err(|error| error.to_string())?;
            let mongodb::bson::Bson::Document(document) = bson else {
                return Err("each imported value must be a document".into());
            };
            documents.push(
                mongodb::bson::to_bson(&document)
                    .map_err(|error| error.to_string())?
                    .into_canonical_extjson()
                    .to_string(),
            );
        }
        self.import_draft = Some(MongoImportDraft {
            documents,
            accepted: false,
        });
        Ok(())
    }

    pub fn accept_import_draft(&mut self) -> bool {
        let Some(draft) = self.import_draft.as_mut() else {
            return false;
        };
        draft.accepted = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::{doc, raw::RawDocumentBuf};

    #[test]
    fn rejects_stale_page_and_keeps_current_page_typed() {
        let namespace = MongoNamespace::collection("app", "events.with.dots");
        let mut tab = MongoBrowserTab::new(Uuid::new_v4(), namespace);
        let stale = tab.begin_load();
        let current = tab.begin_load();
        let bson = doc! { "n": 7_i64 };
        let document = crate::db::mongodb::types::MongoDocument {
            raw: RawDocumentBuf::from_bytes(mongodb::bson::to_vec(&bson).unwrap()).unwrap(),
            bson,
            editable: true,
        };
        let page = MongoDocumentPage {
            documents: vec![document],
            next_offset: None,
            truncated: false,
            oversized_document: false,
            bytes: 16,
        };
        assert!(!tab.accept_page(stale, page.clone()));
        assert!(tab.accept_page(current, page));
        assert!(
            tab.selected_document_ejson()
                .unwrap()
                .contains("$numberLong")
        );
    }

    #[test]
    fn mutation_reducer_reports_partial_and_unknown_and_ignores_stale_generation() {
        let mut tab =
            MongoBrowserTab::new(Uuid::new_v4(), MongoNamespace::collection("app", "items"));
        let stale = tab.begin_mutation();
        let current = tab.begin_mutation();
        let partial = crate::db::mongodb::mutation::MongoMutationOutcome::Partial {
            affected: 2,
            requested: 5,
        };
        assert!(!tab.accept_mutation_outcome(stale, &partial));
        assert!(tab.accept_mutation_outcome(current, &partial));
        assert!(tab.mutation_message.as_deref().unwrap().contains("2 of 5"));
        let unknown = crate::db::mongodb::mutation::MongoMutationOutcome::Unknown { requested: 1 };
        assert!(tab.accept_mutation_outcome(current, &unknown));
        assert!(
            tab.mutation_message
                .as_deref()
                .unwrap()
                .contains("Outcome unknown")
        );
    }

    #[test]
    fn edit_round_trip_requires_a_document_and_preserves_extended_values() {
        let mut tab =
            MongoBrowserTab::new(Uuid::new_v4(), MongoNamespace::collection("app", "items"));
        let bson = doc! { "_id": mongodb::bson::oid::ObjectId::new(), "n": 4_i64 };
        let document = crate::db::mongodb::types::MongoDocument {
            raw: RawDocumentBuf::from_bytes(mongodb::bson::to_vec(&bson).unwrap()).unwrap(),
            bson: bson.clone(),
            editable: true,
        };
        tab.page = MongoPageState::Ready(MongoDocumentPage {
            documents: vec![document],
            next_offset: None,
            truncated: false,
            oversized_document: false,
            bytes: 40,
        });
        assert!(tab.begin_edit());
        assert_eq!(tab.edited_document().unwrap(), bson);
        tab.edit_text = "[]".into();
        assert!(tab.edited_document().is_err());
    }

    #[test]
    fn raw_duplicate_key_document_is_kept_read_only() {
        let raw_bytes = [
            19, 0, 0, 0, 0x10, b'x', 0, 1, 0, 0, 0, 0x10, b'x', 0, 2, 0, 0, 0, 0,
        ];
        let raw = mongodb::bson::raw::RawDocumentBuf::from_bytes(raw_bytes.to_vec()).unwrap();
        let raw_field_count = raw.iter().count();
        let bson: mongodb::bson::Document = mongodb::bson::from_slice(raw.as_bytes()).unwrap();
        let mut tab =
            MongoBrowserTab::new(Uuid::new_v4(), MongoNamespace::collection("app", "raw"));
        tab.page = MongoPageState::Ready(MongoDocumentPage {
            documents: vec![crate::db::mongodb::types::MongoDocument {
                bson,
                raw,
                editable: false,
            }],
            next_offset: None,
            truncated: false,
            oversized_document: false,
            bytes: 19,
        });
        assert!(!tab.begin_edit());
        assert_eq!(raw_field_count, 2);
    }

    #[test]
    fn ordered_sort_filter_and_import_drafts_validate_without_mutating_live_page() {
        let mut tab =
            MongoBrowserTab::new(Uuid::new_v4(), MongoNamespace::collection("app", "items"));
        tab.begin_query_edit(MongoQueryField::Sort);
        tab.query_text = r#"{"z":1,"a":-1}"#.into();
        tab.submit_query_edit().unwrap();
        let keys: Vec<_> = tab.spec.sort.as_ref().unwrap().keys().cloned().collect();
        assert_eq!(keys, vec!["z", "a"]);
        assert!(
            tab.set_import_draft(
                r#"[{"_id":{"$oid":"0123456789abcdef01234567"},"n":{"$numberLong":"7"}}]"#
            )
            .is_ok()
        );
        assert_eq!(tab.import_draft.as_ref().unwrap().documents.len(), 1);
        assert!(tab.set_import_draft("[1]").is_err());
    }

    #[test]
    fn replacement_requires_unchanged_id_and_raw_inspector_does_not_decode_duplicates() {
        let raw = RawDocumentBuf::from_bytes(vec![
            19, 0, 0, 0, 0x10, b'x', 0, 1, 0, 0, 0, 0x10, b'x', 0, 2, 0, 0, 0, 0,
        ])
        .unwrap();
        let document: mongodb::bson::Document = mongodb::bson::from_slice(raw.as_bytes()).unwrap();
        let mut tab =
            MongoBrowserTab::new(Uuid::new_v4(), MongoNamespace::collection("app", "items"));
        tab.page = MongoPageState::Ready(MongoDocumentPage {
            documents: vec![crate::db::mongodb::types::MongoDocument {
                bson: document,
                raw,
                editable: false,
            }],
            next_offset: None,
            truncated: false,
            oversized_document: false,
            bytes: 19,
        });
        let (inspector, raw_only) = tab.selected_document_inspector().unwrap();
        assert!(raw_only);
        assert!(inspector.contains("01 00 00 00"));

        let mut typed =
            MongoBrowserTab::new(Uuid::new_v4(), MongoNamespace::collection("app", "items"));
        let bson = mongodb::bson::doc! {"_id": 1_i32};
        typed.page = MongoPageState::Ready(MongoDocumentPage {
            documents: vec![crate::db::mongodb::types::MongoDocument {
                raw: RawDocumentBuf::from_bytes(mongodb::bson::to_vec(&bson).unwrap()).unwrap(),
                bson,
                editable: true,
            }],
            next_offset: None,
            truncated: false,
            oversized_document: false,
            bytes: 13,
        });
        typed.edit_text = r#"{"_id":2}"#.into();
        assert!(typed.edited_document_preserves_id().is_err());
    }

    #[test]
    fn page_export_reports_loaded_count_and_partial_status() {
        let mut tab =
            MongoBrowserTab::new(Uuid::new_v4(), MongoNamespace::collection("app", "items"));
        let bson = doc! {"_id": 1_i32, "n": 7_i64};
        tab.page = MongoPageState::Ready(MongoDocumentPage {
            documents: vec![crate::db::mongodb::types::MongoDocument {
                raw: RawDocumentBuf::from_bytes(mongodb::bson::to_vec(&bson).unwrap()).unwrap(),
                bson,
                editable: true,
            }],
            next_offset: Some(1),
            truncated: false,
            oversized_document: false,
            bytes: 24,
        });
        let (ejson, count, partial) = tab.loaded_page_ejson().unwrap();
        assert_eq!(count, 1);
        assert!(partial);
        assert!(ejson.contains("$numberLong"));
        assert!(ejson.starts_with('['));
    }
}
