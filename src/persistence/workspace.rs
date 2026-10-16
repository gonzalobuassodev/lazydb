use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::db::catalog::{CatalogId, CatalogKind, QualifiedName};
use crate::model::relation::RelationView;
use crate::model::{execution_target::ExecutionTarget, transaction::TransactionMode};

const WORKSPACE_VERSION: u16 = 5;

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("workspace I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("workspace file is invalid: {0}")]
    Decode(#[from] toml::de::Error),
    #[error("workspace serialization failed: {0}")]
    Encode(#[from] toml::ser::Error),
    #[error("workspace version {found} is not supported; expected {expected}")]
    UnsupportedVersion { found: u16, expected: u16 },
    #[error("workspace is already locked by another LazyDB process")]
    Locked,
    #[error("workspace is invalid: {0}")]
    Invalid(String),
}

impl WorkspaceError {
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Io(_) | Self::Encode(_))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WorkspaceFile {
    pub version: u16,
    #[serde(default)]
    pub active_profile: Option<Uuid>,
    #[serde(default)]
    pub profiles: Vec<PersistedProfileWorkspace>,
    #[serde(default)]
    pub active_tab: Option<Uuid>,
    #[serde(default)]
    pub consoles: Vec<PersistedConsole>,
    #[serde(default)]
    pub tabs: Vec<PersistedTab>,
    #[serde(default)]
    pub recent_targets: Vec<ExecutionTarget>,
}

#[derive(Clone, Debug, Deserialize)]
struct LegacyWorkspaceFile {
    #[serde(rename = "version")]
    _version: u16,
    active_console: Uuid,
    consoles: Vec<PersistedConsole>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PersistedConsole {
    pub id: Uuid,
    pub name: String,
    pub sql_file: PathBuf,
    pub target: Option<ExecutionTarget>,
    pub transaction_mode: TransactionMode,
    #[serde(default = "default_open")]
    pub open: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PersistedProfileWorkspace {
    pub profile_id: Uuid,
    pub active_tab: Option<Uuid>,
    pub consoles: Vec<PersistedConsole>,
    pub tabs: Vec<PersistedTab>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PersistedTab {
    Console {
        console_id: Uuid,
    },
    Relation(PersistedRelationTab),
    Dashboard {
        dashboard_id: Uuid,
        #[serde(default)]
        page: crate::model::dashboard::DashboardPage,
        #[serde(default = "default_dashboard_refresh")]
        refresh_enabled: bool,
        #[serde(default)]
        redis_database: Option<u32>,
    },
    RedisBrowser {
        tab_id: Uuid,
        profile_id: Uuid,
        database: u32,
        #[serde(default)]
        pattern: Vec<u8>,
    },
    PrincipalDdl(PersistedPrincipalTab),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PersistedPrincipalTab {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub scope: crate::db::principal::PrincipalScope,
    pub native_id: String,
    #[serde(rename = "principal_kind")]
    pub kind: crate::db::principal::PrincipalKind,
    pub display_name: String,
    pub native_kind: String,
    #[serde(default)]
    pub system: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PersistedRelationTab {
    pub id: Uuid,
    pub object_id: CatalogId,
    pub qualified_name: QualifiedName,
    pub catalog_kind: CatalogKind,
    pub title: String,
    pub view: RelationView,
}

fn default_open() -> bool {
    true
}

fn default_dashboard_refresh() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceSnapshot {
    pub active_profile: Option<Uuid>,
    pub profiles: Vec<PersistedProfileWorkspace>,
    pub sql: Vec<(Uuid, String)>,
    // Kept only until the app snapshot code is migrated in a later task.
    pub active_console: Uuid,
    pub consoles: Vec<PersistedConsole>,
    pub tabs: Vec<PersistedTab>,
    pub recent_targets: Vec<ExecutionTarget>,
}

#[derive(Clone, Debug)]
pub struct WorkspaceStore {
    manifest: PathBuf,
    sql_dir: PathBuf,
}

pub struct WorkspaceLock {
    path: PathBuf,
    _file: File,
}

impl Drop for WorkspaceLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl WorkspaceStore {
    pub fn new(manifest: PathBuf, sql_dir: PathBuf) -> Self {
        Self { manifest, sql_dir }
    }

    pub fn mongodb_store(&self) -> crate::persistence::mongodb_workspace::MongoWorkspaceStore {
        crate::persistence::mongodb_workspace::MongoWorkspaceStore::new(
            self.sql_dir
                .parent()
                .unwrap_or(&self.sql_dir)
                .join("mongodb"),
        )
    }

    pub fn lock(&self) -> Result<WorkspaceLock, WorkspaceError> {
        if let Some(parent) = self.manifest.parent() {
            fs::create_dir_all(parent)?;
        }
        let path = self.manifest.with_extension("lock");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    WorkspaceError::Locked
                } else {
                    WorkspaceError::Io(error)
                }
            })?;
        Ok(WorkspaceLock { path, _file: file })
    }

    pub fn load(&self) -> Result<Option<WorkspaceSnapshot>, WorkspaceError> {
        let contents = match fs::read_to_string(&self.manifest) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let version = toml::from_str::<toml::Value>(&contents)?
            .get("version")
            .and_then(toml::Value::as_integer)
            .and_then(|version| u16::try_from(version).ok())
            .ok_or_else(|| WorkspaceError::Invalid("workspace version is missing".into()))?;
        let (active_profile, active_console, consoles, tabs, profiles, recent_targets) =
            if matches!(version, 3 | 4 | WORKSPACE_VERSION) {
                let file: WorkspaceFile = toml::from_str(&contents)?;
                (
                    file.active_profile,
                    file.active_tab.unwrap_or(Uuid::nil()),
                    file.consoles,
                    file.tabs,
                    file.profiles,
                    file.recent_targets,
                )
            } else if matches!(version, 1 | 2) {
                let mut file: LegacyWorkspaceFile = toml::from_str(&contents)?;
                if version == 1 {
                    for console in &mut file.consoles {
                        console.open = true;
                    }
                }
                let active_console = file.active_console;
                (
                    None,
                    active_console,
                    Vec::new(),
                    Vec::new(),
                    migrate_legacy(file),
                    Vec::new(),
                )
            } else {
                return Err(WorkspaceError::UnsupportedVersion {
                    found: version,
                    expected: WORKSPACE_VERSION,
                });
            };
        let allow_missing_sql = version <= 2;
        for console in profiles
            .iter()
            .flat_map(|profile| profile.consoles.iter())
            .chain(consoles.iter())
        {
            validate_sql_file_path(console)?;
        }
        let sql = profiles
            .iter()
            .flat_map(|profile| profile.consoles.iter())
            .chain(consoles.iter())
            .map(|console| {
                let path = self.sql_dir.join(&console.sql_file);
                let text = match fs::read_to_string(&path) {
                    Ok(text) => text,
                    Err(_error) if allow_missing_sql => String::new(),
                    Err(error) => {
                        return Err(WorkspaceError::Invalid(format!(
                            "failed to read SQL for console {} from {}: {}",
                            console.id,
                            path.display(),
                            error
                        )));
                    }
                };
                Ok((console.id, text))
            })
            .collect::<Result<Vec<_>, WorkspaceError>>()?;
        let snapshot = WorkspaceSnapshot {
            active_profile,
            profiles,
            sql,
            active_console,
            consoles,
            tabs,
            recent_targets,
        };
        validate_snapshot(&snapshot)?;
        Ok(Some(snapshot))
    }

    pub fn save(&self, snapshot: &WorkspaceSnapshot) -> Result<(), WorkspaceError> {
        validate_snapshot(snapshot)?;
        fs::create_dir_all(&self.sql_dir)?;
        if let Some(parent) = self.manifest.parent() {
            fs::create_dir_all(parent)?;
        }
        for (id, text) in &snapshot.sql {
            let path = self.sql_dir.join(format!("{id}.sql"));
            if fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
                continue;
            }
            let temporary = path.with_extension("sql.tmp");
            let mut file = File::create(&temporary)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            fs::rename(temporary, path)?;
        }
        let file = WorkspaceFile {
            version: WORKSPACE_VERSION,
            active_profile: snapshot.active_profile,
            profiles: snapshot.profiles.clone(),
            active_tab: (snapshot.active_console != Uuid::nil()).then_some(snapshot.active_console),
            consoles: snapshot.consoles.clone(),
            tabs: snapshot.tabs.clone(),
            recent_targets: snapshot.recent_targets.clone(),
        };
        let temporary = self.manifest.with_extension("toml.tmp");
        let mut manifest_file = File::create(&temporary)?;
        manifest_file.write_all(toml::to_string_pretty(&file)?.as_bytes())?;
        manifest_file.sync_all()?;
        fs::rename(temporary, &self.manifest)?;
        Ok(())
    }

    pub fn delete_sql_file(&self, id: Uuid) -> Result<(), WorkspaceError> {
        let path = self.sql_dir.join(format!("{id}.sql"));
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

fn migrate_legacy(file: LegacyWorkspaceFile) -> Vec<PersistedProfileWorkspace> {
    let mut profiles = Vec::<PersistedProfileWorkspace>::new();
    for console in file.consoles {
        let profile_id = console
            .target
            .as_ref()
            .map_or(Uuid::nil(), |target| target.profile_id);
        let profile_index = profiles
            .iter()
            .position(|profile| profile.profile_id == profile_id);
        let profile_index = profile_index.unwrap_or_else(|| {
            profiles.push(PersistedProfileWorkspace {
                profile_id,
                active_tab: None,
                consoles: Vec::new(),
                tabs: Vec::new(),
            });
            profiles.len() - 1
        });
        let profile = &mut profiles[profile_index];
        if console.id == file.active_console && console.open {
            profile.active_tab = Some(console.id);
        }
        if console.open {
            profile.tabs.push(PersistedTab::Console {
                console_id: console.id,
            });
        }
        profile.consoles.push(console);
    }
    profiles
}

pub fn validate_snapshot(snapshot: &WorkspaceSnapshot) -> Result<(), WorkspaceError> {
    let mut profile_ids = std::collections::HashSet::new();
    let mut console_ids = std::collections::HashSet::new();
    let mut tab_ids = std::collections::HashSet::new();
    for profile in &snapshot.profiles {
        if !profile_ids.insert(profile.profile_id) {
            return Err(WorkspaceError::Invalid("duplicate profile ID".into()));
        }
        if profile
            .active_tab
            .is_some_and(|id| !profile.tabs.iter().any(|tab| tab_id(tab) == id))
        {
            return Err(WorkspaceError::Invalid("active tab is not open".into()));
        }
        let profile_console_ids = profile
            .consoles
            .iter()
            .map(|console| console.id)
            .collect::<std::collections::HashSet<_>>();
        for console in &profile.consoles {
            validate_sql_file_path(console)?;
            if !console_ids.insert(console.id) {
                return Err(WorkspaceError::Invalid(format!(
                    "duplicate console ID {} (profile {})",
                    console.id, profile.profile_id
                )));
            }
        }
        let mut open_console_ids = std::collections::HashSet::new();
        for tab in &profile.tabs {
            let id = tab_id(tab);
            match tab {
                PersistedTab::Console { console_id } => {
                    if !profile_console_ids.contains(console_id) {
                        return Err(WorkspaceError::Invalid(
                            "console tab references a missing console".into(),
                        ));
                    }
                    if !open_console_ids.insert(*console_id) {
                        return Err(WorkspaceError::Invalid("duplicate open tab ID".into()));
                    }
                }
                PersistedTab::Relation(relation) => {
                    if console_ids.contains(&relation.id) {
                        return Err(WorkspaceError::Invalid(format!(
                            "relation tab {} duplicates a console ID",
                            relation.id
                        )));
                    }
                    if relation.object_id.profile_id() != profile.profile_id {
                        return Err(WorkspaceError::Invalid(
                            "relation belongs to another profile".into(),
                        ));
                    }
                }
                PersistedTab::Dashboard { dashboard_id, .. } => {
                    if console_ids.contains(dashboard_id) {
                        return Err(WorkspaceError::Invalid(format!(
                            "dashboard tab {dashboard_id} duplicates a console ID"
                        )));
                    }
                }
                PersistedTab::RedisBrowser {
                    tab_id, profile_id, ..
                } => {
                    if *profile_id != profile.profile_id {
                        return Err(WorkspaceError::Invalid(
                            "Redis browser belongs to another profile".into(),
                        ));
                    }
                    if console_ids.contains(tab_id) {
                        return Err(WorkspaceError::Invalid(format!(
                            "Redis browser tab {tab_id} duplicates a console ID"
                        )));
                    }
                }
                PersistedTab::PrincipalDdl(tab) => {
                    if tab.profile_id != profile.profile_id {
                        return Err(WorkspaceError::Invalid(
                            "principal DDL tab belongs to another profile".into(),
                        ));
                    }
                    if console_ids.contains(&tab.id) {
                        return Err(WorkspaceError::Invalid(format!(
                            "principal DDL tab {} duplicates a console ID",
                            tab.id
                        )));
                    }
                }
            }
            if !tab_ids.insert(id) {
                return Err(WorkspaceError::Invalid(format!("duplicate tab ID {id}")));
            }
        }
    }
    for console in &snapshot.consoles {
        validate_sql_file_path(console)?;
        if !console_ids.insert(console.id) {
            return Err(WorkspaceError::Invalid(format!(
                "duplicate console ID {} (global workspace)",
                console.id
            )));
        }
    }
    let mut global_tab_ids = std::collections::HashSet::new();
    for tab in &snapshot.tabs {
        let id = tab_id(tab);
        if !global_tab_ids.insert(id) {
            return Err(WorkspaceError::Invalid("duplicate open tab ID".into()));
        }
        if let PersistedTab::Console { console_id } = tab
            && !snapshot
                .consoles
                .iter()
                .any(|console| console.id == *console_id)
        {
            return Err(WorkspaceError::Invalid(
                "console tab references a missing console".into(),
            ));
        }
    }
    if !snapshot.tabs.is_empty()
        && snapshot.active_console != Uuid::nil()
        && !global_tab_ids.contains(&snapshot.active_console)
    {
        return Err(WorkspaceError::Invalid("active tab is not open".into()));
    }
    if snapshot
        .active_profile
        .is_some_and(|id| !profile_ids.contains(&id))
    {
        return Err(WorkspaceError::Invalid("active profile is missing".into()));
    }
    let mut sql_ids = std::collections::HashSet::new();
    for (id, _) in &snapshot.sql {
        if !sql_ids.insert(*id) {
            return Err(WorkspaceError::Invalid(format!(
                "duplicate SQL entry for console {id}"
            )));
        }
        if !console_ids.contains(id) {
            return Err(WorkspaceError::Invalid(format!(
                "SQL entry names unknown console {id}"
            )));
        }
    }
    if sql_ids.len() != console_ids.len() {
        return Err(WorkspaceError::Invalid(
            "every console must have exactly one SQL entry".into(),
        ));
    }
    Ok(())
}

fn validate_sql_file_path(console: &PersistedConsole) -> Result<(), WorkspaceError> {
    if console.sql_file.as_path() != Path::new(&format!("{}.sql", console.id)) {
        return Err(WorkspaceError::Invalid(format!(
            "invalid SQL file for console {}",
            console.id
        )));
    }
    Ok(())
}

pub(crate) fn tab_id(tab: &PersistedTab) -> Uuid {
    match tab {
        PersistedTab::Console { console_id } => *console_id,
        PersistedTab::Relation(relation) => relation.id,
        PersistedTab::Dashboard { dashboard_id, .. } => *dashboard_id,
        PersistedTab::RedisBrowser { tab_id, .. } => *tab_id,
        PersistedTab::PrincipalDdl(tab) => tab.id,
    }
}
