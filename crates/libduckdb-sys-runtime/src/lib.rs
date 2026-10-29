#![allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    clippy::all
)]

#[path = "upstream/arrow_c_data.rs"]
mod arrow_c_data;
#[path = "upstream/error.rs"]
mod error;
mod runtime;
#[path = "upstream/string.rs"]
mod string;

mod bindings {
    use crate::arrow_c_data::{ArrowArray, ArrowSchema};
    include!(concat!(env!("OUT_DIR"), "/runtime_bindings.rs"));
}

pub use arrow_c_data::{ArrowArray, ArrowSchema};
pub use bindings::*;

pub const DuckDBError: bindings::duckdb_state = bindings::duckdb_state_DuckDBError;
pub const DuckDBSuccess: bindings::duckdb_state = bindings::duckdb_state_DuckDBSuccess;

pub use error::*;
pub use runtime::{
    DuckDbRuntimeError, ensure_loaded, loaded_library_path, resolve_library, runtime_version,
};
pub use string::*;
