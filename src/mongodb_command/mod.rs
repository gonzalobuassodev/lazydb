//! Ordered, conservative parsing and authorization for native MongoDB commands.

mod parser;
mod policy;

pub use parser::{
    MongoCommandCapability, MongoCommandError, MongoCommandKind, MongoCommandRisk,
    ParsedMongoCommand, parse_command,
};
pub use policy::{
    MongoAuthorizationError, MongoWritePolicy, authorize_command, authorize_database,
};
