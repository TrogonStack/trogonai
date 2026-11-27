//! An in-memory sandbox provider worker for tests.
//!
//! [`MemoryWorker`] answers the real provider subjects over NATS: it grants leases, keeps each lease's
//! files in a [`MemoryFs`], runs commands from a scripted [`CommandTable`], serves echo terminals through
//! a [`TerminalHub`], and takes part in the worker authority protocol. No container or VM is involved, so
//! brokers, gateways and clients can be tested end to end against it.

mod builtin_handlers;
mod command_ctx;
mod command_handler;
mod command_table;
mod constants;
mod frame_sink;
mod heartbeat;
mod live_output;
mod memory_fs;
mod memory_worker;
mod memory_worker_args;
mod memory_worker_config;
mod memory_worker_error;
mod mutex_ext;
mod out;
mod profile;
mod resolve;
mod run;
mod run_error;
mod terminal_hub;
mod worker;

pub use command_ctx::CommandCtx;
pub use command_handler::CommandHandler;
pub use command_table::CommandTable;
pub use frame_sink::FrameSink;
pub use heartbeat::{build_heartbeat, build_heartbeat_in_regions};
pub use live_output::LiveOutput;
pub use memory_fs::MemoryFs;
pub use memory_worker::MemoryWorker;
pub use memory_worker_args::MemoryWorkerArgs;
pub use memory_worker_config::MemoryWorkerConfig;
pub use memory_worker_error::MemoryWorkerError;
pub use out::out;
pub use profile::PROFILE;
pub use resolve::resolve;
pub use run::run;
pub use run_error::RunError;
pub use terminal_hub::TerminalHub;
