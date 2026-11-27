//! Operator commands for the sandbox stack.
//!
//! Each command takes typed arguments and a [`Console`] to write to, so the same code backs the
//! `trogon-sandbox-admin` binary and its tests.

mod admin_error;
pub mod api_key;
mod artifact_type;
pub mod capability;
mod console;
pub mod constants;
pub mod credential;
pub mod lease;
mod prebuild_request;
pub mod prewarm;
pub mod quota;
pub mod snapshot;
pub mod substrate;
pub mod tag;
pub mod tenant_policy;
pub mod worker;

pub use admin_error::AdminError;
pub use artifact_type::parse_artifact_type;
pub use console::Console;
pub use prebuild_request::PrebuildRequest;
