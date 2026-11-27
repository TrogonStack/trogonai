//! An MCP stdio server that exposes the sandbox broker's lease, command, file, terminal and snapshot operations as
//! tools, scoped by a per-process role policy.

mod ack;
mod auth;
mod auth_error;
mod config;
mod constants;
mod inputs;
mod launch;
mod lock;
mod logging;
mod parse;
mod policy;
mod progress;
mod reply;
mod run;
mod run_error;
mod server;
mod server_state;
mod terminal;
#[cfg(test)]
mod testing;
mod tool_error;

pub use auth::{default_api_key_path, read_api_key_file, resolve_api_key};
pub use auth_error::AuthError;
pub use config::Config;
pub use logging::init_logging;
pub use policy::{
    CommandPolicy, CommandVetoError, ConfinedPath, PathVetoError, Policy, Probe, RootPolicy, RootPolicyError,
    ToolFilter, verify_no_symlinks,
};
pub use run::run;
pub use run_error::RunError;
pub use server::SandboxMcpServer;
pub use server_state::ServerState;
pub use tool_error::ToolError;
