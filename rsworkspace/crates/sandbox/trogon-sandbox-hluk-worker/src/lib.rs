//! Spike: sandbox leases served by hyperlight-unikraft's `hluk`.
//!
//! A lease picks a published hluk runtime image (`bash`, `python` or `node`) and gets host directories for its
//! `/workspace` and `/tmp`. The
//! first lease of a runtime saves its warm snapshot; every command is then one `hluk run` that restores that
//! snapshot, mounts the workspace at `/workspace` and a per-lease scratch directory at `/tmp`, and runs the command to
//! completion. No guest stays up between commands, so those two directories are the only state a lease keeps.

mod build_heartbeat;
mod constants;
mod guest_dir;
mod guest_network;
mod hluk_cli;
mod hluk_cli_error;
mod hluk_lease;
mod hluk_provider;
mod hluk_runtime;
mod hluk_spec_error;
mod lease_files;
mod lease_workspace;
mod profile;
mod serve;
mod worker_args;

pub use build_heartbeat::build_heartbeat;
pub use guest_dir::GuestDir;
pub use guest_network::GuestNetwork;
pub use hluk_cli::HlukCli;
pub use hluk_cli_error::HlukCliError;
pub use hluk_lease::HlukLease;
pub use hluk_provider::HlukProvider;
pub use hluk_runtime::HlukRuntime;
pub use hluk_spec_error::HlukSpecError;
pub use lease_workspace::LeaseWorkspace;
pub use profile::PROFILE;
pub use serve::serve;
pub use worker_args::WorkerArgs;
