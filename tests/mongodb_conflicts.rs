use lazydb::db::mongodb::mutation::{MongoCompareAndSetOutcome, compare_and_set_outcome};
use mongodb::bson::{Bson, doc};

#[test]
fn compare_and_set_reports_conflict_and_noop_separately_from_applied() {
    assert_eq!(
        compare_and_set_outcome(0, Some(0)),
        MongoCompareAndSetOutcome::Conflict
    );
    assert_eq!(
        compare_and_set_outcome(1, Some(0)),
        MongoCompareAndSetOutcome::Conflict
    );
    assert_eq!(
        compare_and_set_outcome(1, Some(1)),
        MongoCompareAndSetOutcome::Applied
    );
    assert_eq!(
        compare_and_set_outcome(1, None),
        MongoCompareAndSetOutcome::Unknown
    );
}

#[test]
fn exact_guard_preserves_bson_numeric_type_and_nested_structure() {
    let expected = doc! {
        "i32": Bson::Int32(7),
        "i64": Bson::Int64(7),
        "nested": Bson::Array(vec![Bson::Null, Bson::Document(doc! { "ok": true })]),
    };
    assert_eq!(expected.get("i32"), Some(&Bson::Int32(7)));
    assert_eq!(expected.get("i64"), Some(&Bson::Int64(7)));
    assert_eq!(
        expected.get("nested"),
        Some(&Bson::Array(vec![
            Bson::Null,
            Bson::Document(doc! { "ok": true })
        ]))
    );
}
