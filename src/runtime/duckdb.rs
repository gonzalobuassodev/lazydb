//! DuckDB database roots and console session leases.
//!
//! A root belongs to one profile connection generation. Each execution target
//! console gets a stable cloned connection, so switching away and back keeps
//! session state while separate consoles do not share TEMP objects.

use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    db::{DatabaseConnection, DatabaseError},
    model::{execution_target::ExecutionTarget, workspace::ConnectionIdentity},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SessionKey {
    identity: ConnectionIdentity,
    target: ExecutionTarget,
    console_id: Uuid,
}

#[derive(Default)]
pub(crate) struct DuckDbSessions {
    roots: HashMap<ConnectionIdentity, DatabaseConnection>,
    sessions: HashMap<SessionKey, DatabaseConnection>,
}

impl DuckDbSessions {
    pub(crate) fn root_connected(
        &mut self,
        identity: ConnectionIdentity,
        connected: DatabaseConnection,
    ) -> DatabaseConnection {
        if connected.kind() != crate::profile::DatabaseKind::DuckDb {
            return connected;
        }
        self.roots.entry(identity).or_insert(connected).clone()
    }

    pub(crate) fn has_session(
        &self,
        identity: ConnectionIdentity,
        target: &ExecutionTarget,
        console_id: Uuid,
    ) -> bool {
        self.sessions.keys().any(|key| {
            key.identity == identity && &key.target == target && key.console_id == console_id
        })
    }

    pub(crate) fn root(&self, identity: ConnectionIdentity) -> Option<DatabaseConnection> {
        self.roots.get(&identity).cloned()
    }

    pub(crate) fn console_session_for_profile(
        &self,
        identity: ConnectionIdentity,
        console_id: Uuid,
    ) -> Option<(ExecutionTarget, DatabaseConnection)> {
        self.sessions
            .iter()
            .find(|(key, _)| key.identity == identity && key.console_id == console_id)
            .map(|(key, session)| (key.target.clone(), session.clone()))
    }

    #[cfg(test)]
    pub(crate) fn has_console_session(
        &self,
        identity: ConnectionIdentity,
        target: &ExecutionTarget,
        console_id: Uuid,
    ) -> bool {
        self.sessions.contains_key(&SessionKey {
            identity,
            target: target.clone(),
            console_id,
        })
    }

    pub(crate) fn console_session(
        &mut self,
        identity: ConnectionIdentity,
        target: ExecutionTarget,
        console_id: Uuid,
        connected: DatabaseConnection,
    ) -> Result<DatabaseConnection, DatabaseError> {
        let key = SessionKey {
            identity,
            target,
            console_id,
        };
        if let Some(session) = self.sessions.get(&key) {
            return Ok(session.clone());
        }
        if connected.kind() != crate::profile::DatabaseKind::DuckDb {
            return Ok(connected);
        }
        let root = self
            .roots
            .entry(identity)
            .or_insert_with(|| connected.clone())
            .clone();
        let session = root
            .duckdb_session_clone()?
            .expect("DuckDB connection must yield a DuckDB session clone");
        self.sessions.insert(key, session.clone());
        Ok(session)
    }

    pub(crate) fn remove_console(
        &mut self,
        identity: ConnectionIdentity,
        console_id: Uuid,
    ) -> Vec<DatabaseConnection> {
        let keys = self
            .sessions
            .keys()
            .filter(|key| key.identity == identity && key.console_id == console_id)
            .cloned()
            .collect::<Vec<_>>();
        keys.into_iter()
            .filter_map(|key| self.sessions.remove(&key))
            .collect()
    }

    pub(crate) fn remove_console_if_idle(
        &mut self,
        identity: ConnectionIdentity,
        console_id: Uuid,
    ) -> Vec<DatabaseConnection> {
        if self
            .sessions
            .keys()
            .any(|key| key.identity == identity && key.console_id == console_id)
        {
            self.remove_console(identity, console_id)
        } else {
            Vec::new()
        }
    }

    #[cfg(test)]
    pub(crate) fn console_count(&self, identity: ConnectionIdentity) -> usize {
        self.sessions
            .keys()
            .filter(|key| key.identity == identity)
            .map(|key| key.console_id)
            .collect::<std::collections::HashSet<_>>()
            .len()
    }

    pub(crate) fn remove_generation(
        &mut self,
        identity: ConnectionIdentity,
    ) -> Vec<DatabaseConnection> {
        let keys = self
            .sessions
            .keys()
            .filter(|key| key.identity == identity)
            .cloned()
            .collect::<Vec<_>>();
        let mut databases = keys
            .into_iter()
            .filter_map(|key| self.sessions.remove(&key))
            .collect::<Vec<_>>();
        databases.extend(self.roots.remove(&identity));
        databases
    }

    pub(crate) fn remove_all(&mut self) -> Vec<DatabaseConnection> {
        let mut databases = self
            .sessions
            .drain()
            .map(|(_, database)| database)
            .collect::<Vec<_>>();
        databases.extend(self.roots.drain().map(|(_, database)| database));
        databases
    }

    #[cfg(test)]
    pub(crate) fn generation_count(&self) -> usize {
        self.roots.len()
    }

    #[cfg(test)]
    pub(crate) fn session_count(&self, identity: ConnectionIdentity) -> usize {
        self.sessions
            .keys()
            .filter(|key| key.identity == identity)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::import_connection_url;
    use uuid::Uuid;

    fn target(profile_id: Uuid, schema: &str) -> ExecutionTarget {
        ExecutionTarget {
            profile_id,
            database: "memory".into(),
            schema: Some(schema.into()),
        }
    }

    #[tokio::test]
    async fn late_cancel_after_console_release_cannot_recreate_a_session() {
        let profile = import_connection_url("duckdb::memory:", Some("late-cancel"))
            .unwrap()
            .profile;
        let identity = ConnectionIdentity {
            profile_id: profile.id,
            generation: 12,
        };
        let root = DatabaseConnection::connect(&profile, None).await.unwrap();
        let mut sessions = DuckDbSessions::default();
        let target = target(profile.id, "main");
        let console = Uuid::new_v4();
        sessions
            .console_session(identity, target.clone(), console, root.clone())
            .unwrap();
        let removed = sessions.remove_console(identity, console);
        assert_eq!(removed.len(), 1);
        for database in removed {
            database.close().await;
        }
        // A duplicate asynchronous close must not erase a new lease created
        // after the original console was released and reopened.
        assert!(
            sessions
                .console_session(identity, target.clone(), console, root.clone())
                .is_ok()
        );
        assert!(sessions.has_console_session(identity, &target, console));
        assert!(sessions.remove_console_if_idle(identity, console).len() == 1);
        for database in sessions.remove_generation(identity) {
            database.close().await;
        }
        assert_eq!(sessions.generation_count(), 0);
        root.close().await;
    }

    #[tokio::test]
    async fn targets_share_generation_database_but_keep_stable_isolated_sessions() {
        let profile = import_connection_url("duckdb::memory:", Some("sessions"))
            .unwrap()
            .profile;
        let identity = ConnectionIdentity {
            profile_id: profile.id,
            generation: 7,
        };
        let root = DatabaseConnection::connect(&profile, None).await.unwrap();
        let mut sessions = DuckDbSessions::default();
        let first_target = target(profile.id, "main");
        let second_target = target(profile.id, "other");
        let first_id = Uuid::new_v4();
        let first = sessions
            .console_session(identity, first_target.clone(), first_id, root.clone())
            .unwrap();
        first
            .execute("CREATE TABLE shared_values (id INTEGER); INSERT INTO shared_values VALUES (42); CREATE TEMP TABLE only_first (id INTEGER)")
            .await
            .unwrap();
        let second = sessions
            .console_session(identity, second_target, Uuid::new_v4(), root.clone())
            .unwrap();
        let rows = second
            .execute("SELECT id FROM shared_values")
            .await
            .unwrap();
        assert_eq!(rows.result_sets[0].rows.len(), 1);
        assert!(second.execute("SELECT * FROM only_first").await.is_err());

        let first_again = sessions
            .console_session(identity, first_target, first_id, root)
            .unwrap();
        assert!(
            first_again
                .execute("SELECT * FROM only_first")
                .await
                .is_ok()
        );
        assert_eq!(sessions.session_count(identity), 2);

        drop(first);
        drop(second);
        drop(first_again);
        for database in sessions.remove_generation(identity) {
            database.close().await;
        }
        assert_eq!(sessions.session_count(identity), 0);
        assert_eq!(sessions.generation_count(), 0);
    }

    #[tokio::test]
    async fn consoles_with_same_target_have_isolated_temp_objects() {
        let profile = import_connection_url("duckdb::memory:", Some("console-sessions"))
            .unwrap()
            .profile;
        let identity = ConnectionIdentity {
            profile_id: profile.id,
            generation: 2,
        };
        let root = DatabaseConnection::connect(&profile, None).await.unwrap();
        let mut sessions = DuckDbSessions::default();
        let target = target(profile.id, "main");
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let first = sessions
            .console_session(identity, target.clone(), first_id, root.clone())
            .unwrap();
        let second = sessions
            .console_session(identity, target.clone(), second_id, root.clone())
            .unwrap();
        first.execute("CREATE TABLE shared_rows (id INTEGER); INSERT INTO shared_rows VALUES (1); CREATE TEMP TABLE first_temp (id INTEGER); SET TimeZone = 'UTC'").await.unwrap();
        assert_eq!(
            second
                .execute("SELECT id FROM shared_rows")
                .await
                .unwrap()
                .result_sets[0]
                .rows
                .len(),
            1
        );
        assert!(second.execute("SELECT * FROM first_temp").await.is_err());
        let reused = sessions
            .console_session(identity, target.clone(), first_id, root.clone())
            .unwrap();
        assert!(reused.execute("SELECT * FROM first_temp").await.is_ok());

        // The MANUAL worker leases the same console key used by regular SQL.
        let manual_lease = sessions
            .console_session(identity, target.clone(), first_id, root.clone())
            .unwrap();
        let worker = manual_lease
            .start_transaction_worker_with_forced_close(
                crate::runtime::transaction::ForcedCloseHandle::new(),
            )
            .await
            .unwrap();
        let crate::runtime::transaction::TransactionWorkerHandle {
            requests,
            worker,
            readiness,
            ..
        } = worker;
        tokio::time::timeout(std::time::Duration::from_secs(5), readiness)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let (reply, result) = tokio::sync::oneshot::channel();
        requests
            .send(crate::db::transaction::TransactionRequest::Execute {
                query_generation: 1,
                sql: "SELECT count(*) AS n FROM first_temp".into(),
                cancel: tokio::sync::oneshot::channel().1,
                reply,
            })
            .unwrap();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(5), result)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .result_sets[0]
                .rows
                .len(),
            1
        );
        let (reply, result) = tokio::sync::oneshot::channel();
        requests
            .send(crate::db::transaction::TransactionRequest::Rollback { reply })
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), result)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(requests);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap();
        assert!(sessions.has_console_session(identity, &target, first_id));
        let removed = sessions.remove_console(identity, first_id);
        assert_eq!(removed.len(), 1);
        assert!(!sessions.has_console_session(identity, &target, first_id));
        assert!(sessions.has_console_session(identity, &target, second_id));
        assert_eq!(sessions.console_count(identity), 1);
        drop(first);
        drop(second);
        for database in sessions.remove_generation(identity) {
            database.close().await;
        }
        root.close().await;
    }
}
