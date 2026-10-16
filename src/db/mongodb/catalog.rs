use futures_util::TryStreamExt;
use mongodb::{bson::Document, results::CollectionType};

use crate::db::DatabaseError;

use super::{
    MongoAdapter,
    types::{
        DatabaseListAuthority, MongoCollectionInfo, MongoCollectionType, MongoDatabaseListing,
        MongoIndexInfo, MongoNamespace,
    },
};

impl MongoAdapter {
    pub async fn list_databases(&self) -> Result<Vec<String>, DatabaseError> {
        self.list_databases_with_authority(DatabaseListAuthority::AuthorizedOnly)
            .await
            .map(|listing| listing.databases)
    }

    pub async fn list_databases_with_authority(
        &self,
        authority: DatabaseListAuthority,
    ) -> Result<MongoDatabaseListing, DatabaseError> {
        let authorized_only = authority == DatabaseListAuthority::AuthorizedOnly;
        match self
            .client()
            .list_database_names()
            .authorized_databases(authorized_only)
            .await
        {
            Ok(names) => Ok(MongoDatabaseListing {
                databases: names,
                authorized_only,
                warning: None,
            }),
            Err(error) if authorized_only && is_command_unsupported(&error) => {
                Ok(MongoDatabaseListing {
                    databases: vec![self.selected_database().to_owned()],
                    authorized_only: false,
                    warning: Some("server does not support authorized database listing; showing only the selected database".to_owned()),
                })
            }
            Err(error) => Err(super::map_error(error)),
        }
    }

    pub async fn list_collections(
        &self,
        database: &str,
    ) -> Result<Vec<MongoCollectionInfo>, DatabaseError> {
        let cursor = self
            .client()
            .database(database)
            .list_collections()
            .await
            .map_err(super::map_error)?;
        let descriptions = cursor
            .try_collect::<Vec<_>>()
            .await
            .map_err(super::map_error)?;
        descriptions
            .into_iter()
            .map(|description| {
                let collection_type = match description.collection_type {
                    CollectionType::Collection => MongoCollectionType::Collection,
                    CollectionType::View => MongoCollectionType::View,
                    CollectionType::Timeseries => MongoCollectionType::Timeseries,
                    _ => MongoCollectionType::Other,
                };
                let options_ejson = Some(
                    mongodb::bson::to_bson(&description.options)
                        .map_err(|_| {
                            DatabaseError::configuration(
                                "MongoDB collection metadata is not valid BSON",
                            )
                        })?
                        .into_relaxed_extjson()
                        .to_string(),
                );
                Ok(MongoCollectionInfo {
                    namespace: MongoNamespace::collection(database, description.name),
                    collection_type,
                    options_ejson,
                })
            })
            .collect()
    }

    pub async fn list_indexes(
        &self,
        namespace: &MongoNamespace,
    ) -> Result<Vec<MongoIndexInfo>, DatabaseError> {
        let collection = namespace.collection.as_deref().ok_or_else(|| {
            DatabaseError::configuration("MongoDB collection namespace is required")
        })?;
        let cursor = self
            .client()
            .database(&namespace.database)
            .collection::<Document>(collection)
            .list_indexes()
            .await
            .map_err(super::map_error)?;
        cursor
            .try_collect::<Vec<_>>()
            .await
            .map_err(super::map_error)?
            .into_iter()
            .map(|model| {
                let name = model
                    .options
                    .as_ref()
                    .and_then(|options| options.name.clone())
                    .ok_or_else(|| {
                        DatabaseError::configuration("MongoDB returned an index without a name")
                    })?;
                let keys = model.keys.clone();
                let key_description = keys
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                let raw = mongodb::bson::to_document(&model).map_err(|_| {
                    DatabaseError::configuration("MongoDB returned invalid index metadata")
                })?;
                Ok(MongoIndexInfo {
                    name,
                    keys,
                    unique: model
                        .options
                        .as_ref()
                        .and_then(|options| options.unique)
                        .unwrap_or(false),
                    ttl_seconds: model
                        .options
                        .as_ref()
                        .and_then(|options| options.expire_after)
                        .map(|duration| duration.as_secs() as i64),
                    key_description,
                    raw,
                })
            })
            .collect()
    }
}

fn is_command_unsupported(error: &mongodb::error::Error) -> bool {
    matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(command) if command.code == 59 || command.code == 72)
}
