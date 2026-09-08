//! Crate-wide constants.

/// Absolute path to this crate's `wit/` directory.
///
/// `wit_bindgen::generate!` and `wasmtime::component::bindgen!`'s `path:` field resolve
/// relative to the manifest directory of whichever crate is compiling when the macro expands,
/// not this crate's. The `export_decider!` macro (in `trogon-decider-guest-macros`) reads this
/// constant at macro-expansion time and interpolates it as an absolute path, so a component
/// crate can live at any directory depth relative to `trogon-decider-wit`.
pub const WIT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/wit");
