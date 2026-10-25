use std::{env, path::PathBuf};

use crate::db::DatabaseError;

pub fn ensure_loaded() -> Result<PathBuf, DatabaseError> {
    let configured = env::var_os("LAZYDB_DUCKDB_LIB_DIR").filter(|value| !value.is_empty());
    let configured = configured.map(PathBuf::from);
    let data_directory = crate::persistence::paths::AppPaths::discover()
        .map_err(|error| {
            DatabaseError::configuration(format!(
                "unable to resolve LazyDB data directory: {error}"
            ))
        })?
        .data_dir;
    libduckdb_sys::ensure_loaded(configured.as_deref(), Some(&data_directory)).map_err(|error| {
        let code = match error {
            libduckdb_sys::DuckDbRuntimeError::InvalidPath(_) => "duckdb_runtime_invalid_path",
            libduckdb_sys::DuckDbRuntimeError::NotFound(_) => "duckdb_runtime_missing",
            libduckdb_sys::DuckDbRuntimeError::LoadFailed(_) => "duckdb_runtime_load_failed",
            libduckdb_sys::DuckDbRuntimeError::VersionMismatch { .. } => {
                "duckdb_runtime_version_mismatch"
            }
            libduckdb_sys::DuckDbRuntimeError::MissingSymbol(_) => {
                "duckdb_runtime_missing_symbol"
            }
            libduckdb_sys::DuckDbRuntimeError::RestartRequired { .. } => {
                "duckdb_runtime_restart_required"
            }
        };
        DatabaseError {
            category: crate::db::ErrorCategory::Configuration,
            code: Some(code.into()),
            message: crate::security::sanitize_terminal_text(&format!(
                "{error}. Install a compatible DuckDB 1.5.6 C API shared library and set LAZYDB_DUCKDB_LIB_DIR to its absolute directory."
            )),
            diagnostic: None,
        }
    })
}

pub fn loaded_path() -> Option<PathBuf> {
    libduckdb_sys::loaded_library_path()
}

pub fn resolve_configured_library() -> Result<PathBuf, DatabaseError> {
    let explicit = env::var_os("LAZYDB_DUCKDB_LIB_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let data_directory = crate::persistence::paths::AppPaths::discover()
        .map_err(|error| {
            DatabaseError::configuration(format!(
                "unable to resolve LazyDB data directory: {error}"
            ))
        })?
        .data_dir;
    resolve_library(explicit.as_deref(), Some(&data_directory))
}

fn resolve_library(
    explicit: Option<&std::path::Path>,
    data_directory: Option<&std::path::Path>,
) -> Result<PathBuf, DatabaseError> {
    libduckdb_sys::resolve_library(explicit, data_directory).map_err(|error| DatabaseError {
        category: crate::db::ErrorCategory::Configuration,
        code: Some("duckdb_runtime_missing".into()),
        message: crate::security::sanitize_terminal_text(&error.to_string()),
        diagnostic: None,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn rejects_relative_configured_library_directory() {
        let error = super::resolve_library(Some(Path::new("relative/runtime")), None).unwrap_err();
        assert_eq!(error.code.as_deref(), Some("duckdb_runtime_missing"));
        assert!(error.message.contains("must be absolute"));
    }

    #[test]
    fn reports_the_path_that_runtime_discovery_will_use() {
        let root =
            std::env::temp_dir().join(format!("lazydb-runtime-discovery-{}", std::process::id()));
        let runtime_dir = root.join("drivers/duckdb/1.5.6");
        std::fs::create_dir_all(&runtime_dir).unwrap();
        let filename = if cfg!(target_os = "macos") {
            "libduckdb.dylib"
        } else if cfg!(target_os = "windows") {
            "duckdb.dll"
        } else {
            "libduckdb.so"
        };
        let runtime = runtime_dir.join(filename);
        std::fs::write(&runtime, b"not loaded by discovery").unwrap();
        let paths = crate::persistence::paths::AppPaths {
            config_dir: root.clone(),
            data_dir: root.clone(),
            state_dir: root.clone(),
        };
        let resolved = super::resolve_library(None, Some(&paths.data_dir))
            .unwrap()
            .canonicalize()
            .unwrap();
        assert_eq!(resolved, runtime.canonicalize().unwrap());
        let _ = std::fs::remove_dir_all(root);
    }
}
