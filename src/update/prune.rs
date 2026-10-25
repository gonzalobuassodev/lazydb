use sha2::Digest;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    InstallationManager, InstallationStateSource, PruneMode, PruneReport,
    SystemInstallationStateSource, SystemUpdateFileSystem, UpdateLock, Version, native_data_dir,
    read_release_ownership,
};

pub(super) fn run(args: crate::cli::UpdateArgs) -> anyhow::Result<String> {
    if args.check || args.channel.is_some() || args.allow_downgrade {
        anyhow::bail!("--prune cannot be combined with update check, channel, or downgrade options")
    }
    if args.dry_run && args.yes {
        anyhow::bail!("--dry-run and --yes are mutually exclusive")
    }
    let paths = crate::persistence::paths::AppPaths::discover()?;
    let source = SystemInstallationStateSource {
        path: paths.data_dir.join("install.json"),
        file_system: SystemUpdateFileSystem,
    };
    let Some(state) = source.state()? else {
        return render(
            PruneReport {
                schema: 1,
                mode: if args.yes {
                    PruneMode::Applied
                } else {
                    PruneMode::Preview
                },
                current: None,
                previous: None,
                kept: Vec::new(),
                candidates: Vec::new(),
                skipped: vec!["no native installation state".into()],
                reclaimed_bytes: 0,
                errors: Vec::new(),
            },
            args.json,
        );
    };
    if state.manager != InstallationManager::Native {
        return render(
            PruneReport {
                schema: 1,
                mode: if args.yes {
                    PruneMode::Applied
                } else {
                    PruneMode::Preview
                },
                current: None,
                previous: None,
                kept: Vec::new(),
                candidates: Vec::new(),
                skipped: vec!["installation is not managed by LazyDB native installer".into()],
                reclaimed_bytes: 0,
                errors: Vec::new(),
            },
            args.json,
        );
    }
    let data_dir = native_data_dir(&state)?;
    let _lock = UpdateLock::acquire(&data_dir)?;
    let locked_state = source
        .state()?
        .ok_or_else(|| anyhow::anyhow!("native installation state disappeared during prune"))?;
    if locked_state.manager != InstallationManager::Native
        || locked_state.version != state.version
        || locked_state.target != state.target
        || locked_state.path != state.path
    {
        anyhow::bail!("native installation state changed while acquiring prune lock")
    }
    let active = activated_version(&data_dir);
    if active.as_deref() != Some(state.version.as_str()) {
        return render(
            PruneReport {
                schema: 1,
                mode: if args.yes {
                    PruneMode::Applied
                } else {
                    PruneMode::Preview
                },
                current: active,
                previous: None,
                kept: Vec::new(),
                candidates: Vec::new(),
                skipped: vec![
                    "current launcher and installation state disagree; no releases were considered"
                        .into(),
                ],
                reclaimed_bytes: 0,
                errors: Vec::new(),
            },
            args.json,
        );
    }
    let current = activated_version(&data_dir);
    if current.as_deref() != Some(locked_state.version.as_str()) {
        anyhow::bail!("current launcher changed while acquiring prune lock")
    }
    let releases = data_dir.join("releases");
    let mut report = plan(
        &data_dir,
        &releases,
        current.as_deref(),
        Some(&state.target),
        args.yes,
    )?;
    if args.yes {
        apply(&releases, &mut report)?;
    }
    render(report, args.json)
}

fn plan(
    data_dir: &Path,
    releases: &Path,
    current: Option<&str>,
    target: Option<&str>,
    apply: bool,
) -> anyhow::Result<PruneReport> {
    plan_with_probe(
        data_dir,
        releases,
        current,
        target,
        apply,
        process_reference,
    )
}

fn plan_with_probe<P>(
    data_dir: &Path,
    releases: &Path,
    current: Option<&str>,
    target: Option<&str>,
    apply: bool,
    process_probe: P,
) -> anyhow::Result<PruneReport>
where
    P: Fn(&Path) -> ProcessReference,
{
    let mut report = PruneReport {
        schema: 1,
        mode: if apply {
            PruneMode::Applied
        } else {
            PruneMode::Preview
        },
        current: current.map(str::to_owned),
        previous: None,
        kept: Vec::new(),
        candidates: Vec::new(),
        skipped: Vec::new(),
        reclaimed_bytes: 0,
        errors: Vec::new(),
    };
    let entries = match fs::read_dir(releases) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            report
                .skipped
                .push("native releases directory is missing".into());
            return Ok(report);
        }
        Err(error) => return Err(error.into()),
    };

    let mut owned = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = entry.file_type()?;
        if !file_type.is_dir() || file_type.is_symlink() {
            report
                .skipped
                .push(format!("{name}: not a regular release directory"));
            continue;
        }
        match read_release_ownership(&path) {
            Ok(marker)
                if marker.version == name
                    && Version::parse(&name).is_ok()
                    && target.is_none_or(|expected| marker.target == expected) =>
            {
                let bytes = marker.files.iter().map(|file| file.size).sum::<u64>();
                owned.push((name, path, marker.previous_version, bytes));
            }
            Ok(_) => report
                .skipped
                .push(format!("{name}: release marker identity mismatch")),
            Err(error) => report
                .skipped
                .push(format!("{name}: unowned or unverifiable ({error})")),
        }
    }
    owned.sort_by(|left, right| left.0.cmp(&right.0));

    let previous = current.and_then(|version| {
        owned
            .iter()
            .find(|(name, _, _, _)| name == version)
            .and_then(|(_, _, previous, _)| previous.clone())
    });
    report.previous = previous.clone();

    if current.is_some() && previous.is_none() {
        report.skipped.push(
            "rollback chain is unknown; all managed releases are conservatively retained".into(),
        );
        report
            .kept
            .extend(owned.iter().map(|(name, _, _, _)| name.clone()));
        return Ok(report);
    }

    for (name, path, _, bytes) in owned {
        if current == Some(name.as_str()) || previous.as_deref() == Some(name.as_str()) {
            report.kept.push(name);
            continue;
        }
        if let Some(running) = current_executable_release()
            && path.canonicalize().ok().as_ref() == Some(&running)
        {
            report.kept.push(name);
            continue;
        }
        match process_probe(&path) {
            ProcessReference::Referenced => {
                report.kept.push(name);
                continue;
            }
            ProcessReference::Unknown(reason) => {
                report
                    .skipped
                    .push(format!("{name}: process use is unknown ({reason})"));
                continue;
            }
            ProcessReference::Clear => {}
        }
        let _ = data_dir;
        report.reclaimed_bytes = report.reclaimed_bytes.saturating_add(bytes);
        report.candidates.push(name);
    }
    Ok(report)
}

fn apply(releases: &Path, report: &mut PruneReport) -> anyhow::Result<()> {
    apply_with_probe(releases, report, process_reference)
}

fn apply_with_probe<P>(
    releases: &Path,
    report: &mut PruneReport,
    process_probe: P,
) -> anyhow::Result<()>
where
    P: Fn(&Path) -> ProcessReference,
{
    let candidates = report.candidates.clone();
    let planned_bytes = report.reclaimed_bytes;
    report.reclaimed_bytes = 0;
    for version in candidates {
        let path = releases.join(&version);
        let marker = match read_release_ownership(&path) {
            Ok(marker) if marker.version == version => marker,
            Ok(_) => {
                report
                    .skipped
                    .push(format!("{version}: identity changed after preview"));
                continue;
            }
            Err(error) => {
                report.skipped.push(format!(
                    "{version}: ownership changed after preview ({error})"
                ));
                continue;
            }
        };
        if report.current.as_deref() == Some(&version)
            || report.previous.as_deref() == Some(&version)
            || process_probe(&path) != ProcessReference::Clear
        {
            report.skipped.push(format!(
                "{version}: release became protected before deletion"
            ));
            continue;
        }
        let quarantine = releases.join(format!(".prune-{}-{}", marker.version, unique_nonce()));
        fs::rename(&path, &quarantine)?;
        let quarantine_metadata = fs::symlink_metadata(&quarantine)?;
        if !quarantine.starts_with(releases)
            || !quarantine_metadata.file_type().is_dir()
            || quarantine_metadata.file_type().is_symlink()
            || quarantine
                .parent()
                .and_then(|parent| parent.canonicalize().ok())
                != releases.canonicalize().ok()
        {
            report
                .skipped
                .push(format!("{version}: quarantine escaped release root"));
            continue;
        }
        if process_probe(&quarantine) != ProcessReference::Clear {
            fs::rename(&quarantine, &path)?;
            report.skipped.push(format!(
                "{version}: release became active during quarantine verification"
            ));
            continue;
        }
        match remove_owned_files(&quarantine, &marker) {
            Ok(()) => {
                report.reclaimed_bytes = report
                    .reclaimed_bytes
                    .saturating_add(marker.files.iter().map(|file| file.size).sum::<u64>());
            }
            Err(error) => {
                report.errors.push(format!(
                    "{version}: deletion stopped in quarantine ({error})"
                ));
                report.reclaimed_bytes = planned_bytes.saturating_sub(report.reclaimed_bytes);
                continue;
            }
        }
    }
    Ok(())
}

pub(super) fn remove_owned_files(
    directory: &Path,
    marker: &super::ReleaseOwnership,
) -> anyhow::Result<()> {
    for file in &marker.files {
        let path = directory.join(&file.path);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_file() || metadata.len() != file.size {
            anyhow::bail!("owned file changed before deletion")
        }
        let digest = format!("{:x}", sha2::Sha256::digest(fs::read(&path)?));
        if digest != file.sha256 {
            anyhow::bail!("owned file hash changed before deletion")
        }
        fs::remove_file(path)?;
    }
    fs::remove_file(directory.join(".release.json"))?;
    fs::remove_dir(directory)?;
    Ok(())
}

fn activated_version(data_dir: &Path) -> Option<String> {
    let current = fs::read_link(data_dir.join("current")).ok()?;
    let path = if current.is_absolute() {
        current
    } else {
        data_dir.join(current)
    };
    (path.parent()?.file_name()? == "releases")
        .then(|| path.file_name()?.to_str().map(str::to_owned))?
}

fn current_executable_release() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?.canonicalize().ok()?;
    let release = executable.parent()?;
    (release.parent()?.file_name()? == "releases").then(|| release.to_owned())
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ProcessReference {
    Clear,
    Referenced,
    Unknown(String),
}

fn process_reference(release: &Path) -> ProcessReference {
    #[cfg(unix)]
    {
        let output = match Command::new("lsof")
            .args(["+D", &release.to_string_lossy()])
            .output()
        {
            Ok(output) => output,
            Err(error) => return ProcessReference::Unknown(error.to_string()),
        };
        if !output.stderr.is_empty() {
            return ProcessReference::Unknown(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            );
        }
        if output.status.success() && String::from_utf8_lossy(&output.stdout).lines().count() > 1 {
            ProcessReference::Referenced
        } else if output.status.code() == Some(1) && output.stdout.is_empty() {
            ProcessReference::Clear
        } else {
            ProcessReference::Unknown("lsof returned an incomplete process list".into())
        }
    }
    #[cfg(windows)]
    {
        let _ = release;
        ProcessReference::Unknown("Windows process reference probing is not implemented".into())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = release;
        ProcessReference::Unknown("unsupported platform".into())
    }
}

fn unique_nonce() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{nanos}", std::process::id())
}

fn render(report: PruneReport, json: bool) -> anyhow::Result<String> {
    if json {
        return Ok(serde_json::to_string(&report)?);
    }
    let mut lines = vec![format!("LazyDB release prune ({:?})", report.mode)];
    lines.push(format!(
        "Current: {}",
        report.current.as_deref().unwrap_or("unknown")
    ));
    lines.push(format!(
        "Previous: {}",
        report.previous.as_deref().unwrap_or("unknown")
    ));
    lines.push(format!("Candidates: {}", report.candidates.len()));
    lines.push(format!(
        "Potentially reclaimable: {} bytes",
        report.reclaimed_bytes
    ));
    for skipped in report.skipped {
        lines.push(format!("Skipped: {skipped}"));
    }
    for error in report.errors {
        lines.push(format!("Error: {error}"));
    }
    if report.mode == PruneMode::Preview {
        lines.push("Preview only. Re-run with --yes to apply eligible deletions.".into());
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::write_release_ownership;
    use std::sync::Mutex;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let releases = data.join("releases");
        fs::create_dir_all(&releases).unwrap();
        let current = releases.join("1.2.0");
        fs::create_dir(&current).unwrap();
        fs::write(current.join("lazydb"), b"current").unwrap();
        write_release_ownership(&current, "1.2.0", "x86_64-apple-darwin", Some("1.1.0")).unwrap();
        let previous = releases.join("1.1.0");
        fs::create_dir(&previous).unwrap();
        fs::write(previous.join("lazydb"), b"previous").unwrap();
        write_release_ownership(&previous, "1.1.0", "x86_64-apple-darwin", Some("1.0.0")).unwrap();
        let old = releases.join("1.0.0");
        fs::create_dir(&old).unwrap();
        fs::write(old.join("lazydb"), b"old").unwrap();
        write_release_ownership(&old, "1.0.0", "x86_64-apple-darwin", None).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&current, data.join("current")).unwrap();
        (temp, data)
    }

    fn native_install_fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().to_path_buf();
        let releases = data.join("releases");
        let current = releases.join("1.2.0");
        let previous = releases.join("1.1.0");
        let old = releases.join("1.0.0");
        for (path, version, prior, content) in [
            (&current, "1.2.0", Some("1.1.0"), b"current".as_slice()),
            (&previous, "1.1.0", Some("1.0.0"), b"previous".as_slice()),
            (&old, "1.0.0", None, b"old".as_slice()),
        ] {
            fs::create_dir_all(path).unwrap();
            fs::write(path.join("lazydb"), content).unwrap();
            write_release_ownership(path, version, "x86_64-apple-darwin", prior).unwrap();
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&current, data.join("current")).unwrap();
            fs::create_dir(data.join("bin")).unwrap();
            std::os::unix::fs::symlink(data.join("current/lazydb"), data.join("bin/lazydb"))
                .unwrap();
        }
        let state = super::super::InstallationState {
            schema: 1,
            product: "lazydb".into(),
            manager: InstallationManager::Native,
            channel: "stable".into(),
            version: "1.2.0".into(),
            target: "x86_64-apple-darwin".into(),
            path: data.join("bin/lazydb"),
            bin_dir: None,
            installed_at: None,
            shell_profiles: Vec::new(),
        };
        fs::write(
            data.join("install.json"),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        (temp, data)
    }

    #[test]
    fn preview_keeps_active_and_previous_and_does_not_delete() {
        let (_temp, data) = fixture();
        let report = plan_with_probe(
            &data,
            &data.join("releases"),
            Some("1.2.0"),
            Some("x86_64-apple-darwin"),
            false,
            |_| ProcessReference::Clear,
        )
        .unwrap();
        assert_eq!(report.kept, ["1.1.0", "1.2.0"]);
        assert_eq!(report.candidates, ["1.0.0"]);
        assert!(data.join("releases/1.0.0/lazydb").exists());
    }

    #[test]
    fn apply_deletes_only_the_owned_candidate_and_is_idempotent() {
        let (_temp, data) = fixture();
        let mut report = plan_with_probe(
            &data,
            &data.join("releases"),
            Some("1.2.0"),
            Some("x86_64-apple-darwin"),
            true,
            |_| ProcessReference::Clear,
        )
        .unwrap();
        apply_with_probe(&data.join("releases"), &mut report, |_| {
            ProcessReference::Clear
        })
        .unwrap();
        assert!(!data.join("releases/1.0.0").exists());
        assert!(data.join("releases/1.1.0/lazydb").exists());
        assert!(data.join("releases/1.2.0/lazydb").exists());
    }

    #[test]
    fn activation_and_previous_release_come_from_current_marker_chain() {
        let (_temp, data) = fixture();
        let active = activated_version(&data);
        assert_eq!(active.as_deref(), Some("1.2.0"));
        assert_ne!(active.as_deref(), Some("0.8.0"));
        let report = plan_with_probe(
            &data,
            &data.join("releases"),
            active.as_deref(),
            Some("x86_64-apple-darwin"),
            false,
            |_| ProcessReference::Clear,
        )
        .unwrap();
        assert_eq!(report.current.as_deref(), Some("1.2.0"));
        assert_eq!(report.previous.as_deref(), Some("1.1.0"));
    }

    #[test]
    fn unknown_files_and_legacy_releases_are_skipped() {
        let (_temp, data) = fixture();
        fs::write(data.join("releases/1.0.0/user.txt"), b"keep").unwrap();
        let legacy = data.join("releases/0.9.0");
        fs::create_dir(&legacy).unwrap();
        fs::write(legacy.join("lazydb"), b"legacy").unwrap();
        let report = plan_with_probe(
            &data,
            &data.join("releases"),
            Some("1.2.0"),
            Some("x86_64-apple-darwin"),
            false,
            |_| ProcessReference::Clear,
        )
        .unwrap();
        assert!(report.candidates.is_empty());
        assert!(
            report
                .skipped
                .iter()
                .any(|entry| entry.starts_with("1.0.0:"))
        );
        assert!(
            report
                .skipped
                .iter()
                .any(|entry| entry.starts_with("0.9.0:"))
        );
    }

    #[test]
    fn process_reference_unknown_or_active_keeps_candidate() {
        let (_temp, data) = fixture();
        for result in [
            ProcessReference::Referenced,
            ProcessReference::Unknown("probe unavailable".into()),
        ] {
            let report = plan_with_probe(
                &data,
                &data.join("releases"),
                Some("1.2.0"),
                Some("x86_64-apple-darwin"),
                false,
                |_| result.clone(),
            )
            .unwrap();
            assert!(report.candidates.is_empty());
            assert!(
                report.kept.contains(&"1.0.0".to_owned())
                    || report.skipped.iter().any(|item| item.starts_with("1.0.0:"))
            );
        }
    }

    #[test]
    fn cli_preview_and_apply_only_touch_fixture_releases() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_temp, data) = native_install_fixture();
        let old = data.join("releases/1.0.0");
        let previous_config = std::env::var_os("LAZYDB_CONFIG_HOME");
        unsafe { std::env::set_var("LAZYDB_CONFIG_HOME", &data) };
        let args = || crate::cli::UpdateArgs {
            check: false,
            channel: None,
            allow_downgrade: false,
            json: true,
            prune: true,
            dry_run: false,
            yes: false,
        };
        let preview: PruneReport = serde_json::from_str(&run(args()).unwrap()).unwrap();
        assert_eq!(preview.mode, PruneMode::Preview);
        assert_eq!(preview.current.as_deref(), Some("1.2.0"));
        assert_eq!(preview.previous.as_deref(), Some("1.1.0"));
        assert!(old.exists(), "preview must not delete the release");
        assert_eq!(preview.schema, 1);
        assert!(preview.candidates.contains(&"1.0.0".to_owned()));
        assert!(preview.reclaimed_bytes > 0);

        let mut apply_args = args();
        apply_args.yes = true;
        let applied: PruneReport = serde_json::from_str(&run(apply_args).unwrap()).unwrap();
        assert_eq!(applied.mode, PruneMode::Applied);
        assert!(data.join("releases/1.2.0/lazydb").exists());
        assert!(data.join("releases/1.1.0/lazydb").exists());
        assert!(
            !old.exists()
                || applied
                    .skipped
                    .iter()
                    .any(|item| item.starts_with("1.0.0:")),
            "an eligible release must be removed or explicitly skipped"
        );
        assert!(data.join("install.json").is_file());
        match previous_config {
            Some(value) => unsafe { std::env::set_var("LAZYDB_CONFIG_HOME", value) },
            None => unsafe { std::env::remove_var("LAZYDB_CONFIG_HOME") },
        }
    }

    static ENV_LOCK: Mutex<()> = Mutex::new(());
}
