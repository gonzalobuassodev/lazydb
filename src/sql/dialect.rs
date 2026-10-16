#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum SqlDialect {
    Postgres,
    MySql,
    MariaDb,
    SqlServer,
    Sqlite,
    Oracle,
    #[default]
    Generic,
}

impl SqlDialect {
    pub const fn try_for_database_kind(kind: crate::profile::DatabaseKind) -> Option<Self> {
        match kind {
            crate::profile::DatabaseKind::Redis | crate::profile::DatabaseKind::MongoDb => None,
            _ => Some(Self::for_database_kind(kind)),
        }
    }

    pub const fn for_database_kind(kind: crate::profile::DatabaseKind) -> Self {
        match kind {
            crate::profile::DatabaseKind::Postgres => Self::Postgres,
            crate::profile::DatabaseKind::MySql => Self::MySql,
            crate::profile::DatabaseKind::MariaDb => Self::MariaDb,
            crate::profile::DatabaseKind::SqlServer => Self::SqlServer,
            crate::profile::DatabaseKind::Sqlite => Self::Sqlite,
            crate::profile::DatabaseKind::Oracle => Self::Oracle,
            crate::profile::DatabaseKind::Redis => Self::Generic,
            crate::profile::DatabaseKind::MongoDb => Self::Generic,
        }
    }
}

pub(crate) fn parser_dialect(dialect: SqlDialect) -> &'static dyn sqlparser::dialect::Dialect {
    match dialect {
        SqlDialect::Postgres => &sqlparser::dialect::PostgreSqlDialect {},
        SqlDialect::MySql | SqlDialect::MariaDb => &sqlparser::dialect::MySqlDialect {},
        SqlDialect::SqlServer => &sqlparser::dialect::MsSqlDialect {},
        SqlDialect::Sqlite => &sqlparser::dialect::SQLiteDialect {},
        SqlDialect::Generic => &sqlparser::dialect::GenericDialect {},
        SqlDialect::Oracle => &sqlparser::dialect::GenericDialect {},
    }
}
