#[test]
#[cfg(feature = "generator-tests")]
fn generates_a_complete_typed_runtime_table() {
    assert_eq!(libduckdb_sys::REQUIRED_FUNCTIONS.len(), 546);
    assert!(libduckdb_sys::REQUIRED_FUNCTIONS.contains(&"duckdb_library_version"));
    assert!(libduckdb_sys::REQUIRED_FUNCTIONS.contains(&"duckdb_open"));
    assert!(libduckdb_sys::REQUIRED_FUNCTIONS.contains(&"duckdb_destroy_result"));
}

#[test]
#[cfg(feature = "generator-tests")]
fn rejects_unsupported_abi_variadic_and_non_function_items() {
    let bad_abi = "unsafe extern \"system\" { pub fn duckdb_test(); }";
    assert!(generator::parse_bindings(bad_abi).is_err());

    let variadic = "unsafe extern \"C\" { pub fn duckdb_test(value: i32, ...); }";
    assert!(generator::parse_bindings(variadic).is_err());

    let non_function = "unsafe extern \"C\" { static mut duckdb_test: i32; }";
    assert!(generator::parse_bindings(non_function).is_err());

    let unsupported_pattern =
        "unsafe extern \"C\" { pub fn duckdb_test((left, right): (i32, i32)); }";
    assert!(generator::parse_bindings(unsupported_pattern).is_err());
}

#[test]
#[cfg(feature = "generator-tests")]
fn rejects_non_deterministic_or_empty_foreign_declaration_sets() {
    assert!(generator::parse_bindings("pub struct NoFunctions;").is_err());
    let source = include_str!("../src/upstream/bindgen_bundled_version.rs");
    let parsed = generator::parse_bindings(source).unwrap();
    assert_eq!(parsed.1.len(), libduckdb_sys::REQUIRED_FUNCTIONS.len());
    assert_eq!(parsed.1.len(), 546);
}
#[path = "../src/generator.rs"]
mod generator;
