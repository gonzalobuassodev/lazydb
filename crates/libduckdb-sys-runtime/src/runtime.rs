use std::{
    ffi::CStr,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use libloading::Library;

use crate::bindings::{RuntimeFunctions, load_functions};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DuckDbRuntimeError {
    InvalidPath(String),
    NotFound(String),
    LoadFailed(String),
    VersionMismatch {
        found: String,
        required: &'static str,
    },
    MissingSymbol(String),
    RestartRequired {
        loaded: PathBuf,
        requested: PathBuf,
    },
}

impl std::fmt::Display for DuckDbRuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPath(path) => write!(
                formatter,
                "DuckDB runtime directory must be absolute: {path}"
            ),
            Self::NotFound(path) => write!(
                formatter,
                "DuckDB 1.5.6 C API library was not found at {path}"
            ),
            Self::LoadFailed(message) => {
                write!(formatter, "unable to load DuckDB runtime: {message}")
            }
            Self::VersionMismatch { found, required } => write!(
                formatter,
                "DuckDB runtime version {found} is incompatible; required {required}"
            ),
            Self::MissingSymbol(name) => write!(
                formatter,
                "DuckDB runtime is missing required C API symbol {name}"
            ),
            Self::RestartRequired { loaded, requested } => write!(
                formatter,
                "DuckDB runtime {} is already loaded; restart LazyDB before switching to {}",
                loaded.display(),
                requested.display()
            ),
        }
    }
}

impl std::error::Error for DuckDbRuntimeError {}

struct LoadedRuntime {
    _library: Library,
    path: PathBuf,
    functions: RuntimeFunctions,
}

static LOADED: OnceLock<LoadedRuntime> = OnceLock::new();
static LOAD_LOCK: Mutex<()> = Mutex::new(());

pub fn ensure_loaded(
    explicit_directory: Option<&Path>,
    application_data_directory: Option<&Path>,
) -> Result<PathBuf, DuckDbRuntimeError> {
    ensure_loaded_with(explicit_directory, application_data_directory)
}

fn ensure_loaded_with(
    explicit_directory: Option<&Path>,
    application_data_directory: Option<&Path>,
) -> Result<PathBuf, DuckDbRuntimeError> {
    let requested = resolve_library(explicit_directory, application_data_directory)?;
    if let Some(loaded) = LOADED.get() {
        return if loaded.path == requested {
            Ok(loaded.path.clone())
        } else {
            Err(DuckDbRuntimeError::RestartRequired {
                loaded: loaded.path.clone(),
                requested,
            })
        };
    }

    let _guard = LOAD_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(loaded) = LOADED.get() {
        return if loaded.path == requested {
            Ok(loaded.path.clone())
        } else {
            Err(DuckDbRuntimeError::RestartRequired {
                loaded: loaded.path.clone(),
                requested,
            })
        };
    }

    // Loading executes native initializers. Callers only pass an explicit user
    // path or a fixed platform directory, and the handle stays alive forever.
    #[cfg(windows)]
    let library = unsafe {
        let path = requested.as_os_str();
        libloading::os::windows::Library::load_with_flags(
            path,
            libloading::os::windows::LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR
                | libloading::os::windows::LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
        .map(Into::into)
    }
    .map_err(|error: libloading::Error| DuckDbRuntimeError::LoadFailed(error.to_string()))?;
    #[cfg(not(windows))]
    let library = unsafe { Library::new(&requested) }
        .map_err(|error| DuckDbRuntimeError::LoadFailed(error.to_string()))?;
    let version = unsafe { read_version(&library)? };
    let normalized = version.strip_prefix('v').unwrap_or(&version);
    if normalized != "1.5.6" {
        return Err(DuckDbRuntimeError::VersionMismatch {
            found: version,
            required: "1.5.6",
        });
    }

    let functions = load_functions(&library).map_err(DuckDbRuntimeError::MissingSymbol)?;
    let loaded = LoadedRuntime {
        _library: library,
        path: requested.clone(),
        functions,
    };
    LOADED.set(loaded).map_err(|_| {
        DuckDbRuntimeError::LoadFailed("runtime initialization raced unexpectedly".into())
    })?;
    Ok(requested)
}

pub(crate) fn functions() -> &'static RuntimeFunctions {
    &LOADED
        .get()
        .expect("DuckDB runtime functions used before initialization")
        .functions
}

pub fn runtime_version() -> Option<String> {
    let function = functions().duckdb_library_version;
    let pointer = unsafe { function() };
    if pointer.is_null() {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned(),
    )
}

pub fn loaded_library_path() -> Option<PathBuf> {
    LOADED.get().map(|loaded| loaded.path.clone())
}

unsafe fn read_version(library: &Library) -> Result<String, DuckDbRuntimeError> {
    let version: libloading::Symbol<unsafe extern "C" fn() -> *const i8> = unsafe {
        library
            .get(b"duckdb_library_version\0")
            .map_err(|_| DuckDbRuntimeError::MissingSymbol("duckdb_library_version".into()))?
    };
    let pointer = unsafe { version() };
    if pointer.is_null() {
        Ok(String::new())
    } else {
        Ok(unsafe { CStr::from_ptr(pointer.cast()) }
            .to_string_lossy()
            .into_owned())
    }
}

pub fn resolve_library(
    explicit_directory: Option<&Path>,
    application_data_directory: Option<&Path>,
) -> Result<PathBuf, DuckDbRuntimeError> {
    let filename = if cfg!(target_os = "macos") {
        "libduckdb.dylib"
    } else if cfg!(target_os = "windows") {
        "duckdb.dll"
    } else {
        "libduckdb.so"
    };
    let mut directories = Vec::new();
    if let Some(directory) = explicit_directory {
        if !directory.is_absolute() {
            return Err(DuckDbRuntimeError::InvalidPath(
                directory.display().to_string(),
            ));
        }
        directories.push(directory.to_path_buf());
    } else {
        if let Some(data_directory) = application_data_directory {
            directories.push(data_directory.join("drivers/duckdb/1.5.6"));
        } else if let Some(data_dir) = dirs::data_dir() {
            directories.push(data_dir.join("lazydb/drivers/duckdb/1.5.6"));
        }
        #[cfg(target_os = "macos")]
        directories.extend([
            PathBuf::from("/opt/homebrew/lib"),
            PathBuf::from("/usr/local/lib"),
        ]);
        #[cfg(target_os = "linux")]
        {
            directories.extend([
                PathBuf::from("/usr/lib"),
                PathBuf::from("/usr/lib64"),
                PathBuf::from("/lib"),
                PathBuf::from("/lib64"),
            ]);
            #[cfg(target_arch = "x86_64")]
            directories.extend([
                PathBuf::from("/usr/lib/x86_64-linux-gnu"),
                PathBuf::from("/lib/x86_64-linux-gnu"),
            ]);
            #[cfg(target_arch = "aarch64")]
            directories.extend([
                PathBuf::from("/usr/lib/aarch64-linux-gnu"),
                PathBuf::from("/lib/aarch64-linux-gnu"),
            ]);
        }
    }

    for directory in directories {
        if !directory.is_absolute() {
            continue;
        }
        let candidate = directory.join(filename);
        if candidate.is_file() {
            return candidate
                .canonicalize()
                .map_err(|error| DuckDbRuntimeError::LoadFailed(error.to_string()));
        }
    }
    Err(DuckDbRuntimeError::NotFound(filename.into()))
}

#[cfg(test)]
mod tests {
    use super::{DuckDbRuntimeError, ensure_loaded_with, resolve_library};
    use std::{
        path::Path,
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };

    fn library_filename() -> &'static str {
        if cfg!(target_os = "macos") {
            "libduckdb.dylib"
        } else if cfg!(target_os = "windows") {
            "duckdb.dll"
        } else {
            "libduckdb.so"
        }
    }

    static NEXT_RUNTIME: AtomicU64 = AtomicU64::new(0);

    fn temporary_runtime_directory() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "lazydb-duckdb-runtime-{}-{}",
            std::process::id(),
            NEXT_RUNTIME.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn rejects_relative_explicit_directory() {
        assert!(matches!(
            resolve_library(Some(Path::new("relative/runtime")), None),
            Err(DuckDbRuntimeError::InvalidPath(_))
        ));
    }

    #[test]
    fn copied_upstream_helper_and_public_surface_match_pinned_layout() {
        assert!(std::mem::size_of::<crate::ArrowArray>() >= 80);
        assert!(std::mem::size_of::<crate::ArrowSchema>() >= 72);
        assert_eq!(crate::REQUIRED_FUNCTIONS.len(), 546);
        assert!(
            crate::REQUIRED_FUNCTIONS
                .iter()
                .all(|name| name.starts_with("duckdb_"))
        );
    }

    #[test]
    fn explicit_directory_does_not_fall_back() {
        let root = tempdir();
        let missing = root.join("missing-runtime");
        assert!(matches!(
            resolve_library(Some(&missing), None),
            Err(DuckDbRuntimeError::NotFound(_))
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resolves_application_data_directory_before_system_directories() {
        let root = temporary_runtime_directory();
        let candidate = root.join("drivers/duckdb/1.5.6").join(library_filename());
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        std::fs::write(&candidate, b"test library placeholder").unwrap();
        let expected = candidate.canonicalize().unwrap();
        let resolved = resolve_library(None, Some(&root)).unwrap();
        assert_eq!(resolved, expected);
        let _ = std::fs::remove_dir_all(root);
    }

    fn tempdir() -> std::path::PathBuf {
        let path = temporary_runtime_directory();
        std::fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn resolves_versioned_library_path() {
        let root = tempdir();
        let versioned = root.join(if cfg!(target_os = "macos") {
            "libduckdb-v1.5.6.dylib"
        } else if cfg!(target_os = "windows") {
            "duckdb-v1.5.6.dll"
        } else {
            "libduckdb-v1.5.6.so"
        });
        std::fs::write(&versioned, b"not loaded by resolver test").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&versioned, root.join(library_filename())).unwrap();
        #[cfg(windows)]
        std::fs::copy(&versioned, root.join(library_filename())).unwrap();

        let resolved = resolve_library(Some(&root), None).unwrap();
        #[cfg(unix)]
        let expected = versioned.canonicalize().unwrap();
        #[cfg(windows)]
        let expected = root.join(library_filename()).canonicalize().unwrap();
        assert_eq!(resolved, expected);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn failed_load_can_retry_and_success_cannot_hot_switch() {
        let directory = temporary_runtime_directory();
        std::fs::create_dir(&directory).unwrap();
        assert!(matches!(
            ensure_loaded_with(Some(&directory), None),
            Err(DuckDbRuntimeError::NotFound(_))
        ));

        let source = directory.join("stub.c");
        let output = directory.join(library_filename());
        std::fs::write(
            &source,
            "#include <stdbool.h>\n#include <stddef.h>\n#include <stdint.h>\nconst char *duckdb_library_version(void) { return \"v1.5.5\"; }\n",
        )
        .unwrap();
        compile_stub(&source, &output);
        assert!(matches!(
            ensure_loaded_with(Some(&directory), None),
            Err(DuckDbRuntimeError::VersionMismatch { .. })
        ));

        std::fs::write(
            &source,
            "#include <stdbool.h>\n#include <stddef.h>\n#include <stdint.h>\nconst char *duckdb_library_version(void) { return \"v1.5.6\"; }\n",
        )
        .unwrap();
        compile_stub(&source, &output);
        assert!(matches!(
            ensure_loaded_with(Some(&directory), None),
            Err(DuckDbRuntimeError::MissingSymbol(_))
        ));
        std::fs::remove_file(&output).unwrap();
        std::fs::write(
            &source,
            "#include <stdbool.h>\n#include <stddef.h>\n#include <stdint.h>\nconst char *duckdb_library_version(void) { return \"v1.5.5\"; }\n",
        )
        .unwrap();
        compile_stub(&source, &output);
        assert!(matches!(
            ensure_loaded_with(Some(&directory), None),
            Err(DuckDbRuntimeError::VersionMismatch { .. })
        ));
        std::fs::remove_file(&output).unwrap();
        std::fs::write(
            &source,
            "#include <stdbool.h>\n#include <stddef.h>\n#include <stdint.h>\nconst char *duckdb_library_version(void) { return \"v1.5.6\"; }\n",
        )
        .unwrap();
        append_stub_symbols(&source);
        compile_stub(&source, &output);
        let expected = output.canonicalize().unwrap();
        let loaded = ensure_loaded_with(Some(&directory), None).unwrap();
        assert_eq!(loaded, expected);

        let other = directory.join("other");
        std::fs::create_dir(&other).unwrap();
        let other_source = other.join("stub.c");
        let other_output = other.join(library_filename());
        std::fs::write(
            &other_source,
            "#include <stdbool.h>\n#include <stddef.h>\n#include <stdint.h>\nconst char *duckdb_library_version(void) { return \"v1.5.6\"; }\n",
        )
        .unwrap();
        append_stub_symbols(&other_source);
        compile_stub(&other_source, &other_output);
        assert!(matches!(
            ensure_loaded_with(Some(&other), None),
            Err(DuckDbRuntimeError::RestartRequired { .. })
        ));

        let _ = std::fs::remove_dir_all(directory);
    }

    fn compile_stub(source: &Path, output: &Path) {
        let compiler = std::env::var_os("CC").unwrap_or_else(|| "cc".into());
        let mut command = Command::new(compiler);
        if cfg!(target_os = "macos") {
            command.args(["-dynamiclib", "-fPIC"]);
        } else if cfg!(target_os = "windows") {
            command.arg("-shared");
        } else {
            command.args(["-shared", "-fPIC"]);
        }
        let status = command.arg(source).arg("-o").arg(output).status().unwrap();
        assert!(status.success(), "C stub compilation failed");
    }

    fn append_stub_symbols(source: &Path) {
        let mut text = std::fs::read_to_string(source).unwrap();
        for name in crate::REQUIRED_FUNCTIONS {
            if *name != "duckdb_library_version" {
                text.push_str(&format!("void {name}(void) {{}}\n"));
            }
        }
        std::fs::write(source, text).unwrap();
    }
}
