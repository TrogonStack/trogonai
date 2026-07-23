//! Wasm-clean Agent provisioning domain.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod commands;

pub use commands::CommandWireError;
pub use commands::domain;
pub use commands::{ProvisionAgent, ProvisionAgentError};
