#![cfg(not(windows))]

use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn tempdir() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "lazydb-runtime-test-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).unwrap();
    path
}

#[test]
fn loads_the_stub_version_and_complete_symbol_table() {
    let directory = tempdir();
    let source = directory.join("runtime_stub.c");
    let mut source_text = String::from(
        "#include <stdbool.h>\n#include <stddef.h>\n#include <stdint.h>\nconst char *duckdb_library_version(void) { return \"v1.5.6\"; }\n",
    );
    for name in libduckdb_sys::REQUIRED_FUNCTIONS {
        if *name != "duckdb_library_version" {
            source_text.push_str(&format!("void {name}(void) {{}}\n"));
        }
    }
    fs::write(&source, source_text).unwrap();
    let output = directory.join(library_filename());
    compile_stub(&source, &output);

    let loaded = libduckdb_sys::ensure_loaded(Some(&directory), None).unwrap();
    assert_eq!(loaded, output.canonicalize().unwrap());
    let version = libduckdb_sys::runtime_version().unwrap();
    assert_eq!(version, "v1.5.6");
}

#[test]
fn explicit_missing_directory_is_a_configuration_error_without_fallback() {
    let directory = tempdir();
    let child_runtime = directory.join("valid-runtime");
    std::fs::create_dir(&child_runtime).unwrap();
    let mut child_source_text = String::from(
        "#include <stdbool.h>\n#include <stddef.h>\n#include <stdint.h>\nconst char *duckdb_library_version(void) { return \"v1.5.6\"; }\n",
    );
    for name in libduckdb_sys::REQUIRED_FUNCTIONS {
        if *name != "duckdb_library_version" {
            child_source_text.push_str(&format!("void {name}(void) {{}}\n"));
        }
    }
    let child_source_file = child_runtime.join("runtime_stub.c");
    std::fs::write(&child_source_file, child_source_text).unwrap();
    compile_stub(&child_source_file, &child_runtime.join(library_filename()));
    let output = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("child_checks_explicit_missing_runtime")
        .arg("--nocapture")
        .env("LAZYDB_RUNTIME_CHILD", "1")
        .env("LAZYDB_EXPECT_MISSING", "1")
        .env(
            "LAZYDB_DUCKDB_LIB_DIR",
            child_runtime.join("missing").to_str().unwrap(),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok"));

    let output = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("child_checks_explicit_missing_runtime")
        .arg("--nocapture")
        .env("LAZYDB_RUNTIME_CHILD", "1")
        .env("LAZYDB_EXPECT_RUNTIME", "1")
        .env("LAZYDB_DUCKDB_LIB_DIR", child_runtime.to_str().unwrap())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn child_checks_explicit_missing_runtime() {
    if std::env::var_os("LAZYDB_RUNTIME_CHILD").is_some() {
        let configured = std::env::var_os("LAZYDB_DUCKDB_LIB_DIR").map(std::path::PathBuf::from);
        let result = libduckdb_sys::ensure_loaded(configured.as_deref(), None);
        if std::env::var_os("LAZYDB_EXPECT_RUNTIME").is_some() {
            assert!(result.is_ok(), "{}", result.unwrap_err());
        } else if std::env::var_os("LAZYDB_EXPECT_MISSING").is_some() {
            let text = match result {
                Ok(path) => panic!(
                    "explicit missing path unexpectedly loaded {}",
                    path.display()
                ),
                Err(error) => error.to_string(),
            };
            assert!(
                text.contains("DuckDB 1.5.6 C API library was not found"),
                "{text}"
            );
        } else {
            assert!(
                result.is_err(),
                "unexpectedly loaded a runtime without a configured path"
            );
        }
    }
}

fn compile_stub(source: &std::path::Path, output: &std::path::Path) {
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

fn library_filename() -> &'static str {
    if cfg!(target_os = "macos") {
        "libduckdb.dylib"
    } else if cfg!(target_os = "windows") {
        "duckdb.dll"
    } else {
        "libduckdb.so"
    }
}
