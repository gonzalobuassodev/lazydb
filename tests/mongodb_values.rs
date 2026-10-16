use lazydb::db::mongodb::types::MongoDocument;
use mongodb::bson::{Binary, Bson, Decimal128, Document, doc, oid::ObjectId, spec::BinarySubtype};
use std::str::FromStr;

#[test]
fn ejson_round_trip_preserves_extended_bson_types_and_numeric_widths() {
    let original = doc! {
        "_id": ObjectId::parse_str("65a000000000000000000001").unwrap(),
        "i32": Bson::Int32(7),
        "i64": Bson::Int64(7),
        "decimal": Decimal128::from_str("1234567890.123456789").unwrap(),
        "date": mongodb::bson::DateTime::from_millis(1_700_000_000_123),
        "binary": Binary { subtype: BinarySubtype::Uuid, bytes: vec![0, 1, 2, 255] },
        "nested": Bson::Array(vec![Bson::Null, Bson::Document(doc! { "ok": true })]),
    };
    let wrapped = MongoDocument {
        bson: original.clone(),
        raw: mongodb::bson::raw::RawDocumentBuf::from_bytes(
            mongodb::bson::to_vec(&original).unwrap(),
        )
        .unwrap(),
        editable: true,
    };
    let canonical = wrapped.canonical_ejson().unwrap();
    let decoded: Document = serde_json::from_str(&canonical).unwrap();
    assert_eq!(decoded, original);
    assert!(canonical.contains("$oid"));
    assert!(canonical.contains("$numberInt"));
    assert!(canonical.contains("$numberLong"));
    assert!(canonical.contains("$numberDecimal"));
    assert!(canonical.contains("$binary"));
}

#[test]
fn namespace_keeps_dotted_and_unicode_collection_names_verbatim() {
    let namespace = lazydb::db::mongodb::types::MongoNamespace::collection("app", "events.v2_日本");
    assert_eq!(namespace.collection.as_deref(), Some("events.v2_日本"));
}

#[test]
fn find_spec_bounds_page_batch_and_preserves_filter_order() {
    use lazydb::db::mongodb::types::{FindSpec, MongoNamespace};
    let mut spec = FindSpec::new(MongoNamespace::collection("app", "events"));
    spec.page_size = FindSpec::MAX_PAGE_SIZE;
    spec.batch_size = 2;
    spec.sort = Some(doc! { "z": 1, "a": -1 });
    assert!(spec.validate().is_ok());
    assert_eq!(
        spec.sort
            .as_ref()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["z", "a"]
    );
    spec.page_size = FindSpec::MAX_PAGE_SIZE + 1;
    assert!(spec.validate().is_err());
}

#[test]
fn find_page_exposes_byte_budget_and_oversized_flags_without_document_loss() {
    use lazydb::db::mongodb::types::{MongoDocumentPage, MongoNamespace};
    let page = MongoDocumentPage {
        documents: vec![],
        next_offset: Some(0),
        truncated: true,
        oversized_document: true,
        bytes: 0,
    };
    assert!(page.documents.is_empty());
    assert_eq!(page.next_offset, Some(0));
    assert!(page.truncated && page.oversized_document);
    assert_eq!(MongoNamespace::collection("app", "events").database, "app");
}
