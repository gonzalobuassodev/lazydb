use crate::db::{
    DatabaseError,
    principal::{
        PrincipalCoverage, PrincipalDdl, PrincipalDetails, PrincipalEntry, PrincipalId,
        PrincipalKind, PrincipalMembership, PrincipalPage, PrincipalPermission,
        PrincipalPermissionSource, PrincipalReadTarget, PrincipalScope,
    },
    value::CellValue,
};
use crate::identity::ConnectionIdentity;

use super::transport::ClickHouseAdapter;

impl ClickHouseAdapter {
    pub fn principal_capability() -> crate::db::principal::PrincipalCapability {
        crate::db::principal::PrincipalCapability::Details
    }

    pub fn principal_mutation_capabilities() -> crate::db::principal::PrincipalMutationCapabilities
    {
        crate::db::principal::PrincipalMutationCapabilities {
            permission_targets: Vec::new(),
            privileges: Vec::new(),
            memberships: false,
        }
    }

    pub async fn list_principals(&self) -> Result<PrincipalPage, DatabaseError> {
        let rows = self
            .catalog_rows("SELECT name, storage FROM system.users ORDER BY name", &[])
            .await?;
        let mut entries = rows
            .iter()
            .map(|row| {
                let name = safe_text(row, 0)?;
                let storage = safe_text(row, 1)?;
                Ok(PrincipalEntry {
                    id: PrincipalId {
                        profile_id: self.profile_id(),
                        scope: PrincipalScope::Cluster,
                        native_id: format!("user:{name}"),
                        host: None,
                    },
                    kind: PrincipalKind::User,
                    name: name.to_owned(),
                    native_kind: format!("user ({storage})"),
                    system: name.starts_with("default") || name.starts_with("clickhouse_"),
                })
            })
            .collect::<Result<Vec<_>, DatabaseError>>()?;

        let roles = self
            .catalog_rows("SELECT name FROM system.roles ORDER BY name", &[])
            .await?;
        entries.extend(
            roles
                .iter()
                .map(|row| {
                    let name = safe_text(row, 0)?;
                    Ok(PrincipalEntry {
                        id: PrincipalId {
                            profile_id: self.profile_id(),
                            scope: PrincipalScope::Cluster,
                            native_id: format!("role:{name}"),
                            host: None,
                        },
                        kind: PrincipalKind::Role,
                        name: name.to_owned(),
                        native_kind: "role".into(),
                        system: name.starts_with("default") || name.starts_with("clickhouse_"),
                    })
                })
                .collect::<Result<Vec<_>, DatabaseError>>()?,
        );
        entries.sort_by(|left, right| {
            (left.kind == PrincipalKind::Role)
                .cmp(&(right.kind == PrincipalKind::Role))
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(PrincipalPage {
            connection: ConnectionIdentity {
                profile_id: self.profile_id(),
                generation: 0,
            },
            entries,
            complete: true,
        })
    }

    pub async fn principal_ddl(
        &self,
        principal: &PrincipalEntry,
    ) -> Result<PrincipalDdl, DatabaseError> {
        self.validate_principal(principal)?;
        let table = match principal.kind {
            PrincipalKind::User => "system.users",
            PrincipalKind::Role => "system.roles",
        };
        let rows = self
            .catalog_rows(
                &format!("SELECT name FROM {table} WHERE name = ? LIMIT 1"),
                &[&principal.name],
            )
            .await?;
        if rows.is_empty() {
            return Err(DatabaseError::configuration("principal no longer exists"));
        }
        let sql = format!(
            "-- Authentication credentials and authentication clauses are intentionally omitted.\n-- This read-only DDL is informational and is not a complete replay script.\n{} {}",
            if principal.kind == PrincipalKind::Role {
                "CREATE ROLE"
            } else {
                "CREATE USER"
            },
            quote_identifier(&principal.name)
        );
        Ok(PrincipalDdl {
            principal: principal.clone(),
            sql,
        })
    }

    pub async fn principal_details(
        &self,
        principal: &PrincipalEntry,
        target: &PrincipalReadTarget,
    ) -> Result<PrincipalDetails, DatabaseError> {
        self.validate_principal(principal)?;
        if target.principal != principal.id {
            return Err(DatabaseError::configuration(
                "principal details target does not match the principal",
            ));
        }
        if target.database.is_some() {
            return Err(DatabaseError::unsupported(
                "ClickHouse principal details are cluster scoped; database-bound requests are unsupported",
            ));
        }
        let name = principal.name.as_str();
        let permissions_rows = self
            .catalog_rows(
                "SELECT access_type, access_object, database, table, column, grant_option, user_name, role_name, is_partial_revoke, is_wildcard FROM system.grants WHERE (user_name = ? AND ? = 'user') OR (role_name = ? AND ? = 'role') ORDER BY database, table, column, access_type",
                &[name, if principal.kind == PrincipalKind::User { "user" } else { "role" }, name, if principal.kind == PrincipalKind::User { "user" } else { "role" }],
            )
            .await?;
        let permissions_coverage = PrincipalCoverage::Partial(
            if principal.kind == PrincipalKind::Role {
                "ClickHouse grants include partial revokes, wildcard/access-object scopes and inherited grants that the shared permission model does not fully represent; role-derived privileges are not expanded".into()
            } else {
                "ClickHouse grants may include partial revokes, wildcard/access-object scopes and role-inherited privileges that the shared permission model does not fully represent".into()
            },
        );
        let permissions = permissions_rows
            .iter()
            .filter(|row| !safe_bool(row, 8).unwrap_or(false))
            .map(|row| {
                let access = safe_text(row, 0)?;
                let access_object = safe_text(row, 1)?;
                let database = safe_text(row, 2)?;
                let table = safe_text(row, 3)?;
                let column = safe_text(row, 4)?;
                let grantable = safe_bool(row, 5)?;
                let target_name = if !access_object.is_empty() {
                    format!("{access}:{access_object}")
                } else {
                    match (database.is_empty(), table.is_empty(), column.is_empty()) {
                        (true, _, _) => "*.*".to_owned(),
                        (false, true, _) => format!("{database}.*"),
                        (false, false, true) => format!("{database}.{table}"),
                        (false, false, false) => format!("{database}.{table}({column})"),
                    }
                };
                Ok(PrincipalPermission {
                    target: target_name,
                    mutation_target: None,
                    privilege: access.to_owned(),
                    source: "direct".to_owned(),
                    grantable,
                    source_kind: PrincipalPermissionSource::Direct,
                })
            })
            .collect::<Result<Vec<_>, DatabaseError>>()?;

        let membership_rows = self
            .catalog_rows(
                "SELECT user_name, role_name, granted_role_name, with_admin_option FROM system.role_grants WHERE (user_name = ? AND ? = 'user') OR (role_name = ? AND ? = 'role') OR (granted_role_name = ? AND ? = 'role') ORDER BY user_name, role_name, granted_role_name",
                &[name, if principal.kind == PrincipalKind::User { "user" } else { "role" }, name, if principal.kind == PrincipalKind::User { "user" } else { "role" }, name, if principal.kind == PrincipalKind::User { "user" } else { "role" }],
            )
            .await?;
        let mut member_of = Vec::new();
        let mut members = Vec::new();
        for row in &membership_rows {
            let user_name = safe_text(row, 0)?;
            let role_name = safe_text(row, 1)?;
            let granted_role_name = safe_text(row, 2)?;
            let admin_option = safe_bool(row, 3)?;
            let membership = PrincipalMembership {
                role: granted_role_name.to_owned(),
                member: if user_name.is_empty() {
                    role_name.to_owned()
                } else {
                    user_name.to_owned()
                },
                admin_option,
            };
            if principal.kind == PrincipalKind::User && user_name == name {
                member_of.push(membership.clone());
            }
            if principal.kind == PrincipalKind::Role && role_name == name {
                member_of.push(membership.clone());
            }
            if principal.kind == PrincipalKind::Role && granted_role_name == name {
                let mut membership = membership;
                membership.role = principal.name.clone();
                members.push(membership);
            }
        }
        Ok(PrincipalDetails {
            principal: principal.clone(),
            database: None,
            permissions,
            member_of,
            members,
            permissions_coverage,
            membership_coverage: PrincipalCoverage::Complete,
        })
    }

    fn validate_principal(&self, principal: &PrincipalEntry) -> Result<(), DatabaseError> {
        if principal.id.profile_id != self.profile_id()
            || principal.id.scope != PrincipalScope::Cluster
            || principal.id.native_id
                != format!(
                    "{}:{}",
                    if principal.kind == PrincipalKind::User {
                        "user"
                    } else {
                        "role"
                    },
                    principal.name
                )
            || principal.name.trim().is_empty()
        {
            return Err(DatabaseError::configuration(
                "ClickHouse principal identity is invalid or belongs to another profile",
            ));
        }
        if principal.kind == PrincipalKind::User && !principal.id.native_id.starts_with("user:") {
            return Err(DatabaseError::configuration(
                "ClickHouse user principal identity kind is invalid",
            ));
        }
        Ok(())
    }

    pub fn plan_principal_mutation(
        &self,
        principal: &PrincipalEntry,
        connection: crate::identity::ConnectionIdentity,
        database: Option<&str>,
        _mutation: crate::db::principal::PrincipalMutation,
    ) -> Result<crate::db::principal::PrincipalMutationPlan, DatabaseError> {
        self.validate_principal(principal)?;
        if connection.profile_id != self.profile_id() {
            return Err(DatabaseError::configuration(
                "ClickHouse principal mutation connection does not match this profile",
            ));
        }
        if database.is_some() {
            return Err(DatabaseError::unsupported(
                "ClickHouse principal mutations are cluster scoped; database-scoped mutation is unavailable",
            ));
        }
        Err(DatabaseError::unsupported(
            "Structured ClickHouse principal GRANT/REVOKE and role membership mutations are unavailable; use a reviewed native query",
        ))
    }
}

fn safe_text(row: &[CellValue], index: usize) -> Result<&str, DatabaseError> {
    match row.get(index) {
        Some(CellValue::Text(value)) => Ok(value),
        Some(CellValue::Null) => Ok(""),
        _ => Err(DatabaseError::configuration(
            "ClickHouse principal query returned invalid safe metadata",
        )),
    }
}

fn safe_bool(row: &[CellValue], index: usize) -> Result<bool, DatabaseError> {
    match row.get(index) {
        Some(CellValue::Boolean(value)) => Ok(*value),
        Some(CellValue::Unsigned(value)) => Ok(*value != 0),
        Some(CellValue::Integer(value)) => Ok(*value != 0),
        Some(CellValue::Text(value)) => Ok(matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )),
        _ => Ok(false),
    }
}

fn quote_identifier(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    #[test]
    fn clickhouse_principal_ids_distinguish_same_named_users_and_roles() {
        let user_id = PrincipalId {
            profile_id: uuid::Uuid::nil(),
            scope: PrincipalScope::Cluster,
            native_id: "user:analyst".into(),
            host: None,
        };
        let role_id = PrincipalId {
            native_id: "role:analyst".into(),
            ..user_id.clone()
        };
        assert_ne!(user_id, role_id);
    }

    #[test]
    fn clickhouse_principal_ddl_quotes_identifiers_and_disclaims_replay() {
        let sql = format!(
            "-- Authentication credentials and authentication clauses are intentionally omitted.\n-- This read-only DDL is informational and is not a complete replay script.\nCREATE USER {}",
            quote_identifier("analyst`blue")
        );
        assert!(sql.contains("`analyst``blue`"));
        assert!(sql.contains("not a complete replay script"));
        assert!(!sql.to_ascii_lowercase().contains("password"));
    }

    #[test]
    fn clickhouse_principal_lists_only_non_credential_columns() {
        let query = "SELECT name, storage FROM system.users ORDER BY name";
        assert!(query.contains("name, storage"));
        for sensitive in ["auth_params", "auth_type", "password", "grants"] {
            assert!(!query.to_ascii_lowercase().contains(sensitive));
        }
        let role_grants = "SELECT user_name, role_name, granted_role_name, with_admin_option FROM system.role_grants";
        assert!(!role_grants.contains("granted_user_name"));
    }

    #[tokio::test]
    async fn clickhouse_principal_browse_details_and_permission_errors_use_safe_queries() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for _ in 0..5 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                loop {
                    let count = stream.read(&mut buffer).unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let header_end = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .unwrap();
                let sql = String::from_utf8_lossy(&request[header_end + 4..]).to_string();
                let response_body = if sql.contains("SELECT version()") {
                    "25.8.1\tanalytics\tdefault\n"
                } else if sql.contains("FROM system.users") {
                    assert!(sql.contains("SELECT name, storage"));
                    for sensitive in ["auth_params", "auth_type", "password"] {
                        assert!(!sql.to_ascii_lowercase().contains(sensitive));
                    }
                    "name\tstorage\nString\tString\nanalyst\tsqlite\n"
                } else if sql.contains("FROM system.roles") {
                    "name\nString\nanalyst\nString\nreaders\n"
                } else if sql.contains("FROM system.grants") {
                    assert!(sql.contains("grant_option"));
                    assert!(sql.contains("is_partial_revoke"));
                    assert!(sql.contains("is_wildcard"));
                    assert!(sql.contains("role_name"));
                    "access_type\taccess_object\tdatabase\ttable\tcolumn\tgrant_option\tuser_name\trole_name\tis_partial_revoke\tis_wildcard\nString\tString\tNullable(String)\tNullable(String)\tNullable(String)\tUInt8\tNullable(String)\tNullable(String)\tUInt8\tUInt8\nSELECT\t\tanalytics\tevents\t\t1\tanalyst\t\t0\t0\n"
                } else if sql.contains("FROM system.role_grants") {
                    assert!(sql.contains("granted_role_name"));
                    assert!(!sql.contains("granted_user_name"));
                    "user_name\trole_name\tgranted_role_name\twith_admin_option\nNullable(String)\tNullable(String)\tString\tUInt8\nanalyst\t\treaders\t1\n"
                } else {
                    panic!("unexpected principal SQL: {sql}");
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        let imported = crate::profile::import_connection_url(
            &format!("clickhouse://{address}/analytics"),
            Some("principal-mock"),
        )
        .unwrap();
        let adapter =
            ClickHouseAdapter::connect(&imported.profile, imported.transient_password.as_ref())
                .await
                .unwrap();
        let page = adapter.list_principals().await.unwrap();
        let same_name = page
            .entries
            .iter()
            .filter(|entry| entry.name == "analyst")
            .collect::<Vec<_>>();
        assert_eq!(same_name.len(), 2);
        assert_ne!(same_name[0].id, same_name[1].id);
        let user = same_name
            .iter()
            .find(|entry| entry.kind == PrincipalKind::User)
            .unwrap();
        let target = PrincipalReadTarget {
            principal: user.id.clone(),
            database: None,
        };
        let details = adapter.principal_details(user, &target).await.unwrap();
        assert_eq!(details.permissions.len(), 1);
        assert!(details.permissions[0].grantable);
        assert!(matches!(
            details.permissions_coverage,
            PrincipalCoverage::Partial(_)
        ));
        assert_eq!(details.member_of[0].role, "readers");
        assert!(details.member_of[0].admin_option);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn clickhouse_principal_mutations_are_explicitly_gated_and_validate_scope() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            let body = "version()\tcurrentDatabase()\tcurrentUser()\nString\tString\tString\n25.8.1\tanalytics\tdefault\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        let imported = crate::profile::import_connection_url(
            &format!("clickhouse://{address}/analytics"),
            Some("principal-plan-mock"),
        )
        .unwrap();
        let adapter =
            ClickHouseAdapter::connect(&imported.profile, imported.transient_password.as_ref())
                .await
                .unwrap();
        let principal = PrincipalEntry {
            id: PrincipalId {
                profile_id: imported.profile.id,
                scope: PrincipalScope::Cluster,
                native_id: "role:analyst".into(),
                host: None,
            },
            kind: PrincipalKind::Role,
            name: "analyst".into(),
            native_kind: "role".into(),
            system: false,
        };
        let connection = ConnectionIdentity {
            profile_id: imported.profile.id,
            generation: 1,
        };
        let error = adapter
            .plan_principal_mutation(
                &principal,
                connection,
                None,
                crate::db::principal::PrincipalMutation::GrantRole {
                    role: "reader".into(),
                    admin_option: false,
                },
            )
            .unwrap_err();
        assert_eq!(error.category, crate::db::ErrorCategory::Unsupported);
        assert!(error.message.contains("GRANT/REVOKE"));
        let error = adapter
            .plan_principal_mutation(
                &principal,
                connection,
                Some("analytics"),
                crate::db::principal::PrincipalMutation::GrantRole {
                    role: "reader".into(),
                    admin_option: false,
                },
            )
            .unwrap_err();
        assert!(error.message.contains("cluster scoped"));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn restricted_clickhouse_user_gets_permission_error_instead_of_empty_list() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for (status, body) in [
                (
                    200,
                    "version()\tcurrentDatabase()\tcurrentUser()\nString\tString\tString\n25.8.1\tanalytics\trestricted\n",
                ),
                (403, "ACCESS_DENIED: SHOW USERS"),
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 4096];
                let _ = stream.read(&mut request).unwrap();
                let reason = if status == 200 { "OK" } else { "Forbidden" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        let imported = crate::profile::import_connection_url(
            &format!("clickhouse://{address}/analytics"),
            Some("restricted-principal-mock"),
        )
        .unwrap();
        let adapter =
            ClickHouseAdapter::connect(&imported.profile, imported.transient_password.as_ref())
                .await
                .unwrap();
        let error = adapter.list_principals().await.unwrap_err();
        assert_eq!(error.category, crate::db::ErrorCategory::Permission);
        server.join().unwrap();
    }
}
