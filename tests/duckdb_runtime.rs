use lazydb::{
    cli::{CLI_API_VERSION, capabilities, doctor_report, version_info},
    db::DatabaseConnection,
    profile::import_connection_url,
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn tempdir() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "lazydb-runtime-integration-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&path).unwrap();
    path
}

#[test]
fn duckdb_missing_runtime_does_not_break_client_commands_or_other_drivers() {
    let executable = std::env::current_exe().unwrap();
    let output = std::process::Command::new(executable)
        .arg("--exact")
        .arg("child_checks_missing_runtime")
        .arg("--nocapture")
        .env("LAZYDB_RUNTIME_CHILD", "missing")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn child_checks_missing_runtime() {
    if std::env::var_os("LAZYDB_RUNTIME_CHILD").as_deref() != Some(std::ffi::OsStr::new("missing"))
    {
        return;
    }
    let home = tempdir();
    let missing = tempdir();
    unsafe {
        std::env::set_var("LAZYDB_CONFIG_HOME", &home);
        std::env::set_var(
            "LAZYDB_DUCKDB_LIB_DIR",
            missing.join("absent").to_str().unwrap(),
        );
    }

    let version_json = serde_json::to_value(version_info()).unwrap();
    let capabilities_json = serde_json::to_value(capabilities()).unwrap();
    let doctor_json = serde_json::to_value(doctor_report(None)).unwrap();
    assert_eq!(version_json["version"], env!("CARGO_PKG_VERSION"));
    assert!(
        capabilities_json["drivers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|driver| driver == "duckdb")
    );
    assert_eq!(doctor_json["ok"], true);
    assert_eq!(doctor_json["cli_api"], CLI_API_VERSION);
    assert_eq!(doctor_json["checks"][3]["status"], "optional");
    let doctor_text = lazydb::cli::render_command(&lazydb::cli::Command::Doctor {
        json: false,
        profile: None,
    })
    .unwrap();
    assert!(doctor_text.contains("duckdb_runtime: optional"));

    let sqlite = import_connection_url("sqlite::memory:", Some("runtime-independent"))
        .unwrap()
        .profile;
    DatabaseConnection::connect(&sqlite, None).await.unwrap();

    let profile = import_connection_url(
        &format!("duckdb:{}", missing.join("must-not-be-created").display()),
        Some("missing-runtime"),
    )
    .unwrap()
    .profile;
    let error = DatabaseConnection::connect(&profile, None)
        .await
        .unwrap_err();
    assert_eq!(error.code.as_deref(), Some("duckdb_runtime_missing"));
    assert!(error.message.contains("LAZYDB_DUCKDB_LIB_DIR"));
    assert!(!missing.join("must-not-be-created").exists());

    let saved = tempdir();
    let target = saved.join("profiles.json");
    std::fs::write(&target, serde_json::to_vec(&profile).unwrap()).unwrap();
    let restored: lazydb::profile::ConnectionProfile =
        serde_json::from_slice(&std::fs::read(target).unwrap()).unwrap();
    assert_eq!(restored, profile);

    unsafe {
        std::env::remove_var("LAZYDB_CONFIG_HOME");
        std::env::remove_var("LAZYDB_DUCKDB_LIB_DIR");
    }
}

#[test]
fn duckdb_loads_the_external_runtime_and_executes_a_query() {
    let configured = std::env::var_os("LAZYDB_TEST_DUCKDB_LIB_DIR")
        .or_else(|| std::env::var_os("LAZYDB_DUCKDB_LIB_DIR"));
    let previous = std::env::var_os("LAZYDB_DUCKDB_LIB_DIR");
    if let Some(path) = configured {
        unsafe { std::env::set_var("LAZYDB_DUCKDB_LIB_DIR", path) };
    }
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("child_checks_external_runtime")
        .arg("--nocapture")
        .env("LAZYDB_RUNTIME_CHILD", "external")
        .output()
        .unwrap();
    if let Some(previous) = previous {
        unsafe { std::env::set_var("LAZYDB_DUCKDB_LIB_DIR", previous) };
    } else {
        unsafe { std::env::remove_var("LAZYDB_DUCKDB_LIB_DIR") };
    }
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
}

#[tokio::test]
async fn child_checks_external_runtime() {
    if std::env::var_os("LAZYDB_RUNTIME_CHILD").as_deref() != Some(std::ffi::OsStr::new("external"))
    {
        return;
    }
    let profile = import_connection_url("duckdb::memory:", Some("external-runtime"))
        .unwrap()
        .profile;
    let database = DatabaseConnection::connect(&profile, None).await.unwrap();
    let result = database.execute("SELECT 42 AS answer").await.unwrap();
    assert_eq!(
        result.result_sets[0].rows[0][0],
        lazydb::db::value::CellValue::Integer(42)
    );
    assert!(lazydb::db::duckdb_runtime::loaded_path().is_some());
    database.close().await;
}
