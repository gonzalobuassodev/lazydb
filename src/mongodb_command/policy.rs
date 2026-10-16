use crate::{
    mongodb_command::{MongoCommandRisk, ParsedMongoCommand},
    profile::{CatalogScope, ConnectionProfile, Environment},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MongoWritePolicy {
    #[default]
    Deny,
    NonProduction,
    All,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum MongoAuthorizationError {
    #[error("Mongo command database is outside the connection scope")]
    DatabaseScope,
    #[error("Mongo command collection namespace is outside the connection scope")]
    NamespaceScope,
    #[error("Mongo command is read-only but requires write authorization")]
    ReadOnlyRequired,
    #[error("connection is configured read-only")]
    ProfileReadOnly,
    #[error("Mongo write policy denies changes")]
    PolicyDenied,
    #[error("production writes require the all policy")]
    ProductionDenied,
}

pub fn authorize_command(
    profile: &ConnectionProfile,
    command: &ParsedMongoCommand,
    policy: MongoWritePolicy,
) -> Result<(), MongoAuthorizationError> {
    if !profile.catalog_scope.allows_database(&command.database) {
        return Err(MongoAuthorizationError::DatabaseScope);
    }
    for namespace in &command.namespaces {
        if namespace.contains('\0') {
            return Err(MongoAuthorizationError::NamespaceScope);
        }
        if let Some((database, _collection)) = namespace.split_once('.') {
            if database != command.database || !profile.catalog_scope.allows_database(database) {
                return Err(MongoAuthorizationError::NamespaceScope);
            }
        } else if command.kind == crate::mongodb_command::MongoCommandKind::RenameCollection {
            return Err(MongoAuthorizationError::NamespaceScope);
        }
    }
    if command.risk != MongoCommandRisk::ReadOnly {
        if profile.read_only {
            return Err(MongoAuthorizationError::ProfileReadOnly);
        }
        if policy == MongoWritePolicy::Deny {
            return Err(MongoAuthorizationError::PolicyDenied);
        }
        if profile.environment == Environment::Production && policy != MongoWritePolicy::All {
            return Err(MongoAuthorizationError::ProductionDenied);
        }
    }
    Ok(())
}

pub fn authorize_database(
    scope: &CatalogScope,
    database: &str,
) -> Result<(), MongoAuthorizationError> {
    scope
        .allows_database(database)
        .then_some(())
        .ok_or(MongoAuthorizationError::DatabaseScope)
}
