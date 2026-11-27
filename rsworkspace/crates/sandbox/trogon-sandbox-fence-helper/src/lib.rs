//! Windows network fence helper.
//!
//! The elevated half of the Windows backend: it provisions the sandbox
//! account, installs the WFP filters, grants and stamps ACLs, and launches a
//! command as the sandbox account. The library exposes the SID, group, and WFP
//! primitives so they can be unit-tested, plus the full CLI dispatch as
//! `run_from_args` so an embedding multicall binary can link the crate
//! directly. The `trogon-sandbox-fence-helper` binary (`main.rs`) is a
//! one-line shim over that entry point.
//!
//! Windows-only. Building on other platforms yields an empty crate so
//! `cargo check` from a non-Windows host doesn't error.

#![cfg(windows)]

pub mod acl;
pub mod ambient;
pub mod cert_store;
pub mod cli;
pub mod context;
pub mod dpapi;
pub mod error;
pub mod install;
pub mod job;
pub mod launch;
pub mod logon;
pub mod path_id;
pub mod persist;
pub mod probe;
pub mod profile;
pub mod profile_plan;
pub mod reg;
pub mod runner;
pub mod sam;
pub mod self_protect;
pub mod sid;
pub mod state_db;
pub mod token;
pub mod user;
pub mod util;
pub mod wfp;
pub mod winsta;

pub use cli::{DISPATCH_ARG1, run_from_args};
pub use context::Context;
pub use error::{FenceHelperError, Result};
