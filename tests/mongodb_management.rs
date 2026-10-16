mod support;

use lazydb::{
    db::{
        DatabaseConnection,
        mongodb::types::{MongoCollectionType, MongoNamespace},
    },
    model::mongodb_management::MongoManagementOperation as Op,
    profile::import_connection_url,
};
use mongodb::bson::{Document, doc};

#[tokio::test]
async fn collection_view_index_management_round_trips_against_mongodb() {
    let Some(url) = support::mongodb_test_url() else {
        return;
    };
    let imported = import_connection_url(&url, Some("management-test")).unwrap();
    let connection =
        DatabaseConnection::connect(&imported.profile, imported.transient_password.as_ref())
            .await
            .unwrap();
    let DatabaseConnection::MongoDb(adapter) = connection else {
        panic!("expected MongoDB adapter")
    };
    let database = format!("lazydb_management_{}", uuid::Uuid::new_v4().simple());
    let collection = MongoNamespace::collection(&database, "events");

    let (created, _) = adapter
        .manage(
            &Op::CreateCollection {
                database: database.clone(),
            },
            doc! { "name": "events", "options": {} },
        )
        .await
        .unwrap();
    assert_eq!(created, collection);
    adapter.manage(&Op::CreateIndex { namespace: collection.clone() }, doc! { "keys": { "kind": 1, "sequence": -1 }, "options": { "name": "kind_sequence", "unique": true } }).await.unwrap();
    let indexes = adapter.list_indexes(&collection).await.unwrap();
    let compound = indexes
        .iter()
        .find(|index| index.name == "kind_sequence")
        .unwrap();
    assert_eq!(
        compound.keys.keys().map(String::as_str).collect::<Vec<_>>(),
        ["kind", "sequence"]
    );
    assert!(compound.unique);

    adapter
        .manage(
            &Op::CreateView {
                database: database.clone(),
            },
            doc! { "name": "event_view", "viewOn": "events", "pipeline": [] },
        )
        .await
        .unwrap();
    assert!(
        adapter
            .list_collections(&database)
            .await
            .unwrap()
            .iter()
            .any(|item| item.collection_type == MongoCollectionType::View
                && item.namespace.collection.as_deref() == Some("event_view"))
    );
    adapter
        .manage(
            &Op::CollMod {
                namespace: collection.clone(),
            },
            doc! { "validator": { "kind": { "$type": "string" } }, "validationLevel": "moderate" },
        )
        .await
        .unwrap();
    let renamed = MongoNamespace::collection(&database, "events_archive");
    adapter
        .manage(
            &Op::RenameCollection {
                namespace: collection,
            },
            doc! { "target": "events_archive" },
        )
        .await
        .unwrap();
    adapter
        .manage(
            &Op::DropCollection {
                namespace: renamed,
                kind: MongoCollectionType::Collection,
            },
            Document::new(),
        )
        .await
        .unwrap();
    adapter
        .manage(
            &Op::DropCollection {
                namespace: MongoNamespace::collection(&database, "event_view"),
                kind: MongoCollectionType::View,
            },
            Document::new(),
        )
        .await
        .unwrap();
    adapter.close().await;
}
