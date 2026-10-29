use std::{collections::BTreeSet, fs, io::Cursor, path::Path};

#[cfg(any(target_os = "linux", target_os = "macos", test))]
use std::io::Read;
#[cfg(any(target_os = "linux", target_os = "macos", test))]
use xz2::read::XzDecoder;

use super::ChannelManifest;

pub(super) fn extract_archive(
    archive: &[u8],
    destination: &Path,
    manifest: &ChannelManifest,
    target: &str,
) -> anyhow::Result<()> {
    let expected_root = format!("lazydb_{}_{}", manifest.version, target);
    let binary_file = if target == "x86_64-pc-windows-msvc" {
        "lazydb.exe"
    } else {
        "lazydb"
    };
    let binary_name = format!("{expected_root}/{binary_file}");
    let mut validator = ArchiveValidation {
        expected_root: &expected_root,
        binary_name: &binary_name,
        names: BTreeSet::new(),
        root_seen: false,
        binary_seen: false,
    };

    #[cfg(all(windows, not(test)))]
    extract_zip(archive, destination, &mut validator)?;
    #[cfg(all(any(target_os = "linux", target_os = "macos"), not(test)))]
    extract_tar_xz(archive, destination, &mut validator)?;
    #[cfg(test)]
    if target == "x86_64-pc-windows-msvc" {
        extract_zip(archive, destination, &mut validator)?;
    } else {
        extract_tar_xz(archive, destination, &mut validator)?;
    }

    if !validator.root_seen || !validator.binary_seen {
        anyhow::bail!("archive does not contain the expected executable")
    }
    fs::rename(
        destination.join(&expected_root),
        destination.join("lazydb-root"),
    )?;
    let root = destination.join("lazydb-root");
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        fs::rename(entry.path(), destination.join(entry.file_name()))?;
    }
    fs::remove_dir(&root)?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(destination.join(binary_file))?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(destination.join(binary_file), permissions)?;
    }
    Ok(())
}

struct ArchiveValidation<'a> {
    expected_root: &'a str,
    binary_name: &'a str,
    names: BTreeSet<String>,
    root_seen: bool,
    binary_seen: bool,
}

fn archive_entry_is_normal(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\0')
        && !name.contains('\\')
        && !name.contains(':')
        && !name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

impl ArchiveValidation<'_> {
    fn entry(&mut self, raw_name: &str, is_dir: bool, is_file: bool) -> anyhow::Result<()> {
        let name = if is_dir {
            raw_name.strip_suffix('/').unwrap_or(raw_name)
        } else {
            raw_name
        };
        if !archive_entry_is_normal(name)
            || !(name == self.expected_root
                || name.starts_with(&format!("{}/", self.expected_root)))
            || !self.names.insert(name.to_owned())
            || !(is_dir || is_file)
            || (name == self.expected_root && !is_dir)
            || (name == self.binary_name && !is_file)
        {
            anyhow::bail!("unsafe archive entry: {raw_name}")
        }
        self.root_seen |= name == self.expected_root && is_dir;
        self.binary_seen |= name == self.binary_name && is_file;
        Ok(())
    }
}

#[cfg(any(windows, test))]
fn extract_zip(
    archive: &[u8],
    destination: &Path,
    validator: &mut ArchiveValidation<'_>,
) -> anyhow::Result<()> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive))?;
    for index in 0..zip.len() {
        let entry = zip.by_index(index)?;
        let mode = entry.unix_mode().unwrap_or(0) & 0o170000;
        if entry.is_symlink() || !matches!(mode, 0 | 0o040000 | 0o100000) {
            anyhow::bail!("unsafe archive entry: {}", entry.name())
        }
        validator.entry(entry.name(), entry.is_dir(), entry.is_file())?;
    }
    if !validator.root_seen || !validator.binary_seen {
        anyhow::bail!("archive does not contain the expected executable")
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(archive))?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let output = destination.join(entry.name());
        if entry.is_dir() {
            fs::create_dir_all(output)?;
        } else {
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)?;
            }
            std::io::copy(&mut entry, &mut fs::File::create(output)?)?;
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn extract_tar_xz(
    archive: &[u8],
    destination: &Path,
    validator: &mut ArchiveValidation<'_>,
) -> anyhow::Result<()> {
    let mut decoder = XzDecoder::new(Cursor::new(archive));
    let mut tar_bytes = Vec::new();
    decoder.read_to_end(&mut tar_bytes)?;
    let mut tar_archive = tar::Archive::new(Cursor::new(&tar_bytes));
    for entry in tar_archive.entries()? {
        let entry = entry?;
        let path = entry.path_bytes();
        let name = std::str::from_utf8(&path)?;
        let entry_type = entry.header().entry_type();
        validator.entry(name, entry_type.is_dir(), entry_type.is_file())?;
    }
    if !validator.root_seen || !validator.binary_seen {
        anyhow::bail!("archive does not contain the expected executable")
    }
    tar::Archive::new(Cursor::new(&tar_bytes)).unpack(destination)?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn archive_entry_path_is_normal(name: &str) -> bool {
    archive_entry_is_normal(name)
}
