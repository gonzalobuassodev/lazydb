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
