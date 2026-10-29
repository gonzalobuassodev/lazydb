use mongodb::bson::{Bson, Document};

use crate::db::DatabaseError;

use super::{MongoAdapter, map_error, types::MongoNamespace};

/// The filter that bounds a document replacement to one `_id`.
///
/// The value is the `_id` exactly as it was loaded, never the editor's text and
/// never a re-serialized rendering, so the write cannot retarget another
/// document. It is the only field in the filter on purpose.
pub fn replace_one_filter(id: &Bson) -> Document {
    mongodb::bson::doc! { "_id": id.clone() }
}

impl MongoAdapter {
    /// Replaces one document, bounded to the original `_id` value.
    ///
    /// This is the whole write surface of the document modal: one
    /// `replaceOne` against `{_id: <original>}`, never a bulk write and never
    /// an update by any other field.
    pub async fn replace_document_by_id_detailed(
        &self,
        namespace: &MongoNamespace,
        id: Bson,
        replacement: Document,
    ) -> Result<MongoMutationOutcome, DatabaseError> {
        let collection = namespace.collection.as_deref().ok_or_else(|| {
            DatabaseError::configuration("MongoDB collection namespace is required")
        })?;
        if replacement.get("_id") != Some(&id) {
            return Err(DatabaseError::configuration(
                "the replacement document must keep the original _id",
            ));
        }
        let result = self
            .client()
            .database(&namespace.database)
            .collection::<Document>(collection)
            .replace_one(replace_one_filter(&id), replacement)
            .await;
        match result {
            Ok(result) if result.matched_count == 1 => {
                Ok(MongoMutationOutcome::Confirmed { affected: 1 })
            }
            Ok(_) => Ok(MongoMutationOutcome::Confirmed { affected: 0 }),
            Err(error) if error_is_definitely_rejected(&error) => Err(map_error(error)),
            Err(_) => Ok(MongoMutationOutcome::Unknown { requested: 1 }),
        }
    }
}

/// The message the tab shows when a replacement did not apply, or `None` when
/// the document really was replaced.
pub fn replace_outcome_message(outcome: &MongoMutationOutcome) -> Option<&'static str> {
    match outcome {
        MongoMutationOutcome::Confirmed { affected: 0 } => {
            Some("the document no longer exists in the collection")
        }
        MongoMutationOutcome::Confirmed { .. } => None,
        MongoMutationOutcome::Partial { .. } | MongoMutationOutcome::Unknown { .. } => {
            Some("MongoDB replacement outcome is unknown; reload the page before retrying")
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(tag = "certainty", rename_all = "snake_case")]
pub enum MongoMutationOutcome {
    Confirmed { affected: u64 },
    Partial { affected: u64, requested: u64 },
    Unknown { requested: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MongoCompareAndSetOutcome {
    Applied,
    Conflict,
    Unknown,
}

pub fn compare_and_set_outcome(matched: u64, modified: Option<u64>) -> MongoCompareAndSetOutcome {
    if matched == 0 {
        MongoCompareAndSetOutcome::Conflict
    } else if modified.is_some_and(|modified| modified == 1) {
        MongoCompareAndSetOutcome::Applied
    } else if modified == Some(0) {
        MongoCompareAndSetOutcome::Conflict
    } else {
        MongoCompareAndSetOutcome::Unknown
    }
}

pub fn error_certainty(error: &mongodb::error::Error) -> MongoMutationOutcome {
    use mongodb::error::{ErrorKind, WriteFailure};
    match error.kind.as_ref() {
        ErrorKind::Command(_)
        | ErrorKind::Write(WriteFailure::WriteError(_))
        | ErrorKind::InvalidArgument { .. }
        | ErrorKind::Authentication { .. }
        | ErrorKind::BsonDeserialization(_)
        | ErrorKind::BsonSerialization(_) => MongoMutationOutcome::Confirmed { affected: 0 },
        ErrorKind::Io(_)
        | ErrorKind::ConnectionPoolCleared { .. }
        | ErrorKind::ServerSelection { .. }
        | ErrorKind::InsertMany(_)
        | ErrorKind::Write(WriteFailure::WriteConcernError(_)) => {
            MongoMutationOutcome::Unknown { requested: 1 }
        }
        _ => MongoMutationOutcome::Unknown { requested: 1 },
    }
}

pub fn error_is_definitely_rejected(error: &mongodb::error::Error) -> bool {
    use mongodb::error::{ErrorKind, WriteFailure};
    matches!(
        error.kind.as_ref(),
        ErrorKind::Command(_)
            | ErrorKind::Write(WriteFailure::WriteError(_))
            | ErrorKind::InvalidArgument { .. }
            | ErrorKind::Authentication { .. }
            | ErrorKind::BsonDeserialization(_)
            | ErrorKind::BsonSerialization(_)
    )
}

impl MongoMutationOutcome {
    pub const fn is_fully_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::{MongoCompareAndSetOutcome, MongoMutationOutcome, compare_and_set_outcome};

    #[test]
    fn the_replacement_filter_is_exactly_the_original_id() {
        let id = mongodb::bson::Bson::ObjectId(mongodb::bson::oid::ObjectId::new());
        let filter = super::replace_one_filter(&id);

        assert_eq!(
            filter,
            mongodb::bson::doc! { "_id": id.clone() },
            "the filter is the original _id value and nothing else"
        );
        assert_eq!(
            filter.keys().collect::<Vec<_>>(),
            vec!["_id"],
            "no other field can widen or retarget the write"
        );
        assert!(
            matches!(filter.get("_id"), Some(mongodb::bson::Bson::ObjectId(_))),
            "the _id keeps the BSON type it was loaded with, never a re-serialized string"
        );
    }

    #[test]
    fn certainty_distinguishes_partial_and_unknown_from_confirmed() {
        assert!(MongoMutationOutcome::Confirmed { affected: 2 }.is_fully_confirmed());
        assert!(
            !MongoMutationOutcome::Partial {
                affected: 1,
                requested: 3
            }
            .is_fully_confirmed()
        );
        assert!(!MongoMutationOutcome::Unknown { requested: 1 }.is_fully_confirmed());
    }

    #[test]
    fn cas_requires_one_match_and_distinguishes_conflict_from_noop() {
        assert_eq!(
            compare_and_set_outcome(0, Some(0)),
            MongoCompareAndSetOutcome::Conflict
        );
        assert_eq!(
            compare_and_set_outcome(1, Some(1)),
            MongoCompareAndSetOutcome::Applied
        );
        assert_eq!(
            compare_and_set_outcome(1, Some(0)),
            MongoCompareAndSetOutcome::Conflict
        );
        assert_eq!(
            compare_and_set_outcome(1, None),
            MongoCompareAndSetOutcome::Unknown
        );
    }

    #[test]
    fn insert_many_partial_count_uses_unique_failed_indexes() {
        let failed_indexes = [1usize, 3, 3];
        let affected = 5u64.saturating_sub(
            failed_indexes
                .into_iter()
                .collect::<std::collections::HashSet<_>>()
                .len() as u64,
        );
        assert_eq!(affected, 3);
        assert_eq!(
            MongoMutationOutcome::Partial {
                affected,
                requested: 5
            },
            MongoMutationOutcome::Partial {
                affected: 3,
                requested: 5
            }
        );
    }
}
