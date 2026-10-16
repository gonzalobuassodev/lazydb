mod support;

use lazydb::{
    db::{
        DatabaseConnection,
        mongodb::types::{FindSpec, MongoNamespace},
    },
    profile::import_connection_url,
};
use mongodb::bson::doc;

#[tokio::test]
async fn find_pages_cross_driver_batches_and_keep_bson_documents_raw() {
    let Some(url) = support::mongodb_test_url() else {
        return;
    };
    let imported = import_connection_url(&url, Some("cursor-test")).unwrap();
    let connection =
        DatabaseConnection::connect(&imported.profile, imported.transient_password.as_ref())
            .await
            .unwrap();
    let DatabaseConnection::MongoDb(adapter) = connection else {
        panic!("expected MongoDB")
    };
    let database = format!("lazydb_mongo_test_{}", uuid::Uuid::new_v4().simple());
    let collection = "cursor.batch_test";
    let fixture_client = mongodb::Client::with_uri_str(&url).await.unwrap();
    let fixture_collection = fixture_client
        .database(&database)
        .collection::<mongodb::bson::Document>(collection);
    let docs = (0..350)
        .map(|n| doc! {"n": n, "group": n % 3})
        .collect::<Vec<_>>();
    fixture_collection.insert_many(docs).await.unwrap();

    let mut spec = FindSpec::new(MongoNamespace::collection(&database, collection));
    spec.page_size = 73;
    spec.batch_size = 2;
    spec.sort = Some(doc! {"n": 1});
    let mut next = Some(0u64);
    let mut values = Vec::new();
    while let Some(offset) = next {
        spec.skip = offset;
        let page = adapter.find_page(&spec).await.unwrap();
        values.extend(
            page.documents
                .iter()
                .map(|document| document.bson.get_i32("n").unwrap()),
        );
        next = page.next_offset;
    }
    assert_eq!(values, (0..350).collect::<Vec<_>>());
    assert_eq!(adapter.count_documents(&spec).await.unwrap(), 350);
    fixture_client.database(&database).drop().await.unwrap();
    fixture_client.shutdown().await;
    adapter.close().await;
}
