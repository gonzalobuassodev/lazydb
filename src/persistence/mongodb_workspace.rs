use std::{
    collections::HashSet,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PersistedMongoConsole {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub database: String,
    pub command_file: PathBuf,
    pub open: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PersistedMongoWorkspace {
    pub consoles: Vec<PersistedMongoConsole>,
    #[serde(skip)]
    pub commands: Vec<(Uuid, String)>,
}

#[derive(Debug, Error)]
pub enum MongoWorkspaceError {
    #[error("Mongo workspace I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Mongo workspace manifest is invalid: {0}")]
    Decode(#[from] toml::de::Error),
    #[error("Mongo workspace serialization failed: {0}")]
    Encode(#[from] toml::ser::Error),
    #[error("invalid Mongo command file for console {0}")]
    InvalidPath(Uuid),
}

#[derive(Clone, Debug)]
pub struct MongoWorkspaceStore {
    directory: PathBuf,
}

impl MongoWorkspaceStore {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub fn save(&self, snapshot: &PersistedMongoWorkspace) -> Result<(), MongoWorkspaceError> {
        fs::create_dir_all(&self.directory)?;
        let mut expected = HashSet::new();
        for console in &snapshot.consoles {
            validate_path(console)?;
            expected.insert(console.command_file.clone());
            let command = snapshot
                .commands
                .iter()
                .find(|(id, _)| *id == console.id)
                .map_or("", |(_, value)| value);
            let path = self.directory.join(&console.command_file);
            let temporary = path.with_extension("json.tmp");
            let mut file = File::create(&temporary)?;
            file.write_all(command.as_bytes())?;
            file.sync_all()?;
            fs::rename(temporary, path)?;
        }
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            if entry.file_name() == "workspace.toml" || entry.file_name() == "workspace.toml.tmp" {
                continue;
            }
            if entry.path().extension().is_some_and(|ext| ext == "json")
                && !expected.contains(&PathBuf::from(entry.file_name()))
            {
                fs::remove_file(entry.path())?;
            }
        }
        #[derive(Serialize)]
        struct Manifest<'a> {
            consoles: &'a [PersistedMongoConsole],
        }
        let manifest = self.directory.join("workspace.toml");
        let temporary = self.directory.join("workspace.toml.tmp");
        fs::write(
            &temporary,
            toml::to_string_pretty(&Manifest {
                consoles: &snapshot.consoles,
            })?,
        )?;
        fs::rename(temporary, manifest)?;
        Ok(())
    }

    pub fn load(&self) -> Result<Option<PersistedMongoWorkspace>, MongoWorkspaceError> {
        #[derive(Deserialize)]
        struct Manifest {
            consoles: Vec<PersistedMongoConsole>,
        }
        let path = self.directory.join("workspace.toml");
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let manifest: Manifest = toml::from_str(&text)?;
        let mut snapshot = PersistedMongoWorkspace {
            consoles: manifest.consoles,
            commands: Vec::new(),
        };
        for console in &snapshot.consoles {
            validate_path(console)?;
            snapshot.commands.push((
                console.id,
                fs::read_to_string(self.directory.join(&console.command_file))?,
            ));
        }
        Ok(Some(snapshot))
    }

    pub fn mark_closed(&self, id: Uuid) -> Result<(), MongoWorkspaceError> {
        let Some(mut snapshot) = self.load_manifest()? else {
            return Ok(());
        };
        if let Some(console) = snapshot
            .consoles
            .iter_mut()
            .find(|console| console.id == id)
        {
            console.open = false;
            self.save(&snapshot)?;
        }
        Ok(())
    }

    pub fn delete_console(&self, id: Uuid) -> Result<(), MongoWorkspaceError> {
        let Some(mut snapshot) = self.load_manifest()? else {
            return Ok(());
        };
        if let Some(index) = snapshot
            .consoles
            .iter()
            .position(|console| console.id == id)
        {
            let console = snapshot.consoles.remove(index);
            self.save(&snapshot)?;
            let path = self.directory.join(console.command_file);
            if path.exists() {
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }

    fn load_manifest(&self) -> Result<Option<PersistedMongoWorkspace>, MongoWorkspaceError> {
        #[derive(Deserialize)]
        struct Manifest {
            consoles: Vec<PersistedMongoConsole>,
        }
        let path = self.directory.join("workspace.toml");
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let manifest: Manifest = toml::from_str(&text)?;
        Ok(Some(PersistedMongoWorkspace {
            consoles: manifest.consoles,
            commands: Vec::new(),
        }))
    }
}

fn validate_path(console: &PersistedMongoConsole) -> Result<(), MongoWorkspaceError> {
    if console.command_file.as_path() != Path::new(&format!("{}.mongodb.json", console.id)) {
        return Err(MongoWorkspaceError::InvalidPath(console.id));
    }
    Ok(())
}
