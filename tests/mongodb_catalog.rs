mod support;

use lazydb::{
    db::{
        DatabaseConnection,
        mongodb::types::{MongoCollectionType, MongoNamespace},
    },
    profile::import_connection_url,
};

#[tokio::test]
async fn lists_authorized_databases_collections_views_and_ordered_indexes() {
    let Some(url) = support::mongodb_test_url() else {
        return;
    };
    let imported = import_connection_url(&url, Some("catalog-test")).unwrap();
    let connection =
        DatabaseConnection::connect(&imported.profile, imported.transient_password.as_ref())
            .await
            .unwrap();
    let DatabaseConnection::MongoDb(adapter) = connection else {
        panic!("expected MongoDB adapter")
    };
    let database = format!("lazydb_mongo_test_{}", uuid::Uuid::new_v4().simple());
    let collection_name = "catalog.events_日本";
    let fixture_client = mongodb::Client::with_uri_str(&url).await.unwrap();
    let fixture_collection = fixture_client
        .database(&database)
        .collection::<mongodb::bson::Document>(collection_name);
    fixture_collection
        .insert_one(mongodb::bson::doc! {"kind": "test", "sequence": 1})
        .await
        .unwrap();
    fixture_collection
        .create_index(
            mongodb::IndexModel::builder()
                .keys(mongodb::bson::doc! {"kind": 1, "sequence": -1})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .name("kind_sequence".to_owned())
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    let databases = adapter.list_databases().await.unwrap();
    assert!(databases.contains(&database.to_owned()));
    let collections = adapter.list_collections(&database).await.unwrap();
    assert!(
        collections
            .iter()
            .any(
                |entry| entry.namespace.collection.as_deref() == Some(collection_name)
                    && entry.collection_type == MongoCollectionType::Collection
            )
    );
    let indexes = adapter
        .list_indexes(&MongoNamespace::collection(&database, collection_name))
        .await
        .unwrap();
    let compound = indexes
        .iter()
        .find(|index| index.name == "kind_sequence")
        .unwrap();
    assert_eq!(
        compound
            .key_description()
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>(),
        vec!["kind", "sequence"]
    );
    fixture_client.database(&database).drop().await.unwrap();
    fixture_client.shutdown().await;
    adapter.close().await;
}
