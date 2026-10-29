use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::fs;

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct MigrationReport {
    pub status: &'static str,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub launcher: Option<PathBuf>,
    pub compatibility_link: Option<PathBuf>,
    pub warnings: Vec<String>,
}

pub fn run(args: crate::cli::MigrateHomeArgs) -> anyhow::Result<String> {
    #[cfg(not(unix))]
    {
        let _ = args;
        anyhow::bail!("migrate-home is currently supported on Unix only")
    }

    #[cfg(unix)]
    {
        let paths = crate::persistence::paths::AppPaths::discover()?;
        let source = paths.config_dir;
        let destination = absolute_path(&args.to)?;
        let launcher = native_launcher(&source)?;
        let report = MigrationReport {
            status: if args.dry_run { "planned" } else { "complete" },
            source: source.clone(),
            destination: destination.clone(),
            launcher: launcher.clone(),
            compatibility_link: Some(source.clone()),
            warnings: vec![
                "stop LazyDB, MCP, and LSP processes before migration".to_owned(),
                "the migration does not merge an existing destination".to_owned(),
            ],
        };
        validate(&source, &destination, launcher.as_deref())?;
        if args.dry_run {
            return render(&report, args.json);
        }
        if !args.yes {
            anyhow::bail!("migrate-home requires --yes or --dry-run")
        }
        migrate(&source, &destination, launcher.as_deref())?;
        render(&report, args.json)
    }
}

fn render(report: &MigrationReport, json: bool) -> anyhow::Result<String> {
    if json {
        Ok(serde_json::to_string(report)?)
    } else {
        let mut output = format!(
            "LazyDB home migration {}\nsource: {}\ndestination: {}\n",
            report.status,
            report.source.display(),
            report.destination.display()
        );
        if let Some(launcher) = &report.launcher {
            output.push_str(&format!("launcher: {}\n", launcher.display()));
        }
        for warning in &report.warnings {
            output.push_str(&format!("warning: {warning}\n"));
        }
        Ok(output)
    }
}

fn absolute_path(path: &Path) -> anyhow::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

#[cfg(unix)]
fn native_launcher(source: &Path) -> anyhow::Result<Option<PathBuf>> {
    let state_path = source.join("install.json");
    if !state_path.is_file() {
        return Ok(None);
    }
    let input = fs::read_to_string(&state_path)?;
    let state = crate::update::parse_installation_state(&input)?;
    if state.manager != crate::update::InstallationManager::Native {
        return Ok(None);
    }
    Ok(Some(state.path))
}

#[cfg(unix)]
fn validate(source: &Path, destination: &Path, launcher: Option<&Path>) -> anyhow::Result<()> {
    if !source.is_dir() {
        anyhow::bail!(
            "source LazyDB directory does not exist: {}",
            source.display()
        )
    }
    if source == destination {
        anyhow::bail!("source and destination are identical")
    }
    if destination.exists() || fs::symlink_metadata(destination).is_ok() {
        anyhow::bail!("destination already exists: {}", destination.display())
    }
    if destination.starts_with(source) || source.starts_with(destination) {
        anyhow::bail!("source and destination cannot be nested")
    }
    if let Some(parent) = destination.parent() {
        let parent_metadata = parent
            .metadata()
            .map_err(|error| anyhow::anyhow!("destination parent is unavailable: {error}"))?;
        let source_device = std::os::unix::fs::MetadataExt::dev(&fs::metadata(source)?);
        let parent_device = std::os::unix::fs::MetadataExt::dev(&parent_metadata);
        if source_device != parent_device {
            anyhow::bail!("source and destination are on different filesystems")
        }
    }
    if let Some(launcher) = launcher
        && !fs::symlink_metadata(launcher)?.file_type().is_symlink()
    {
        anyhow::bail!("native launcher is not a symlink: {}", launcher.display())
    }
    if launcher.is_some() && !source.join("current").is_symlink() {
        anyhow::bail!("native installation is missing its current link")
    }
    Ok(())
}

#[cfg(unix)]
fn migrate(source: &Path, destination: &Path, launcher: Option<&Path>) -> anyhow::Result<()> {
    let lock_path = source.with_extension("migration.lock");
    let _lock = crate::update::UpdateLock::acquire_path(lock_path)?;
    validate(source, destination, launcher)?;
    let state = source.join("install.json");
    let version = if state.is_file() {
        let parsed = crate::update::parse_installation_state(&fs::read_to_string(&state)?)?;
        Some(parsed.version)
    } else {
        None
    };
    if let Some(version) = version {
        let current = source.join("current");
        let current_new = source.join(".current.migrate.new");
        #[cfg(unix)]
        std::os::unix::fs::symlink(Path::new("releases").join(version), &current_new)?;
        fs::rename(&current, source.join(".current.migrate.old"))?;
        fs::rename(&current_new, &current)?;
    }
    fs::rename(source, destination)?;
    if let Some(launcher) = launcher {
        let target = destination.join("current/lazydb");
        let temporary = launcher.with_extension(format!("migrate.{}", std::process::id()));
        std::os::unix::fs::symlink(target, &temporary)?;
        fs::rename(temporary, launcher)?;
    }
    std::os::unix::fs::symlink(destination, source)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{absolute_path, migrate, validate};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn relative_destination_is_resolved_from_cwd() {
        assert!(
            absolute_path(std::path::Path::new("lazydb"))
                .unwrap()
                .is_absolute()
        );
    }

    #[cfg(unix)]
    #[test]
    fn existing_destination_is_rejected_without_changes() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        assert!(validate(&source, &destination, None).is_err());
        assert!(source.is_dir());
        assert!(destination.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn migration_moves_complete_root_and_leaves_compatibility_link() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        fs::create_dir_all(source.join("releases/1.0.0")).unwrap();
        fs::write(source.join("connections.toml"), "profiles").unwrap();
        fs::write(source.join("releases/1.0.0/lazydb"), "binary").unwrap();
        std::os::unix::fs::symlink(
            std::path::Path::new("releases/1.0.0"),
            source.join("current"),
        )
        .unwrap();

        migrate(&source, &destination, None).unwrap();
        assert!(destination.join("connections.toml").is_file());
        assert_eq!(
            fs::read_link(destination.join("current")).unwrap(),
            std::path::PathBuf::from("releases/1.0.0")
        );
        assert_eq!(fs::read_link(&source).unwrap(), destination);
    }
}
