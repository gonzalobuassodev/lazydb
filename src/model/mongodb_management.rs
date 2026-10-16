use uuid::Uuid;

use crate::db::mongodb::types::{MongoCollectionType, MongoNamespace};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MongoManagementOperation {
    CreateCollection {
        database: String,
    },
    CreateView {
        database: String,
    },
    RenameCollection {
        namespace: MongoNamespace,
    },
    DropCollection {
        namespace: MongoNamespace,
        kind: MongoCollectionType,
    },
    CollMod {
        namespace: MongoNamespace,
    },
    CreateIndex {
        namespace: MongoNamespace,
    },
    DropIndex {
        namespace: MongoNamespace,
        name: String,
    },
}

impl MongoManagementOperation {
    pub fn is_destructive(&self) -> bool {
        matches!(
            self,
            Self::RenameCollection { .. } | Self::DropCollection { .. } | Self::DropIndex { .. }
        )
    }

    pub fn namespace(&self) -> Option<&MongoNamespace> {
        match self {
            Self::RenameCollection { namespace }
            | Self::DropCollection { namespace, .. }
            | Self::CollMod { namespace }
            | Self::CreateIndex { namespace }
            | Self::DropIndex { namespace, .. } => Some(namespace),
            Self::CreateCollection { .. } | Self::CreateView { .. } => None,
        }
    }

    pub fn database(&self) -> &str {
        self.namespace()
            .map(|namespace| namespace.database.as_str())
            .unwrap_or_else(|| match self {
                Self::CreateCollection { database } | Self::CreateView { database } => database,
                _ => unreachable!("all other operations have a namespace"),
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MongoManagementDraft {
    pub profile_id: Uuid,
    pub generation: u64,
    pub operation: MongoManagementOperation,
    pub json: String,
    pub input: crate::model::text_input::TextInput,
    pub confirmation_text: Option<String>,
    pub confirmation_input: String,
    pub error: Option<String>,
}

impl MongoManagementDraft {
    pub fn new(profile_id: Uuid, generation: u64, operation: MongoManagementOperation) -> Self {
        let (json, confirmation_text) = match &operation {
            MongoManagementOperation::CreateCollection { .. } => (
                "{\n  \"name\": \"\",\n  \"options\": {}\n}".to_owned(),
                None,
            ),
            MongoManagementOperation::CreateView { .. } => (
                "{\n  \"name\": \"\",\n  \"viewOn\": \"\",\n  \"pipeline\": []\n}".to_owned(),
                None,
            ),
            MongoManagementOperation::RenameCollection { namespace } => (
                "{\n  \"target\": \"\"\n}".to_owned(),
                namespace.collection.clone(),
            ),
            MongoManagementOperation::DropCollection { namespace, .. } => {
                ("{}".to_owned(), namespace.collection.clone())
            }
            MongoManagementOperation::CollMod { namespace } => (
                "{\n  \"validator\": {}\n}".to_owned(),
                namespace.collection.clone(),
            ),
            MongoManagementOperation::CreateIndex { .. } => (
                "{\n  \"keys\": {\n    \"field\": 1\n  },\n  \"options\": {}\n}".to_owned(),
                None,
            ),
            MongoManagementOperation::DropIndex { name, .. } => {
                ("{}".to_owned(), Some(name.clone()))
            }
        };
        let mut input = crate::model::text_input::TextInput::default();
        input.set(json.clone());
        Self {
            profile_id,
            generation,
            operation,
            json,
            input,
            confirmation_text,
            confirmation_input: String::new(),
            error: None,
        }
    }

    pub fn validate(&self) -> Result<mongodb::bson::Document, String> {
        let value: serde_json::Value =
            serde_json::from_str(&self.json).map_err(|error| error.to_string())?;
        let bson = mongodb::bson::Bson::try_from(value).map_err(|error| error.to_string())?;
        bson.as_document()
            .cloned()
            .ok_or_else(|| "Mongo management draft must be a JSON object".into())
    }

    pub fn confirmation_valid(&self) -> bool {
        self.confirmation_text
            .as_ref()
            .is_none_or(|expected| &self.confirmation_input == expected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_requires_valid_json_and_exact_confirmation() {
        let namespace = MongoNamespace::collection("app", "items");
        let mut draft = MongoManagementDraft::new(
            Uuid::new_v4(),
            7,
            MongoManagementOperation::DropCollection {
                namespace,
                kind: MongoCollectionType::Collection,
            },
        );
        assert!(draft.validate().is_ok());
        assert!(!draft.confirmation_valid());
        draft.confirmation_input = "items".into();
        assert!(draft.confirmation_valid());
        draft.json = "[]".into();
        assert!(draft.validate().is_err());
    }

    #[test]
    fn ordered_index_keys_survive_ejson_draft_parsing() {
        let mut draft = MongoManagementDraft::new(
            Uuid::new_v4(),
            1,
            MongoManagementOperation::CreateIndex {
                namespace: MongoNamespace::collection("app", "items"),
            },
        );
        draft.json = r#"{"keys":{"z":1,"a":-1},"options":{"unique":true}}"#.into();
        let value = draft.validate().unwrap();
        let keys = value.get_document("keys").unwrap();
        assert_eq!(
            keys.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["z", "a"]
        );
    }

    #[test]
    fn collmod_uses_collection_confirmation_too() {
        let draft = MongoManagementDraft::new(
            Uuid::new_v4(),
            1,
            MongoManagementOperation::CollMod {
                namespace: MongoNamespace::collection("app", "items"),
            },
        );
        assert_eq!(draft.confirmation_text.as_deref(), Some("items"));
        assert!(!draft.confirmation_valid());
    }
}
