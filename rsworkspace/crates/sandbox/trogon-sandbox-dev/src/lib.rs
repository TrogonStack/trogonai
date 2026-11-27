//! One-command local sandbox stack.
//!
//! `trogon-sandbox-dev` supervises four processes as one group: `nats-server` with
//! JetStream, the broker, a worker (docker, subprocess or memory) and the ws gateway.
//! [`DevArgs::plan`] is the pure part: it turns flags into a [`DevPlan`] (what to spawn,
//! with which arguments and environment) so the wiring can be tested without starting
//! anything. [`run`] starts the plan and supervises it until a shutdown future resolves
//! or one process exits.

mod api_key_provisioning;
mod binary_search;
#[cfg(target_os = "linux")]
mod death_signal;
mod dev_args;
mod dev_error;
mod dev_plan;
mod dev_ready;
mod dev_stack;
mod docker_probe;
mod plan_error;
mod process_spec;
mod readiness;
mod running_process;
#[cfg(target_os = "linux")]
mod spawner;
mod worker_kind;

pub use api_key_provisioning::{generate_api_key, register_api_key, write_api_key_file};
pub use binary_search::find_binary;
pub use dev_args::DevArgs;
pub use dev_error::DevError;
pub use dev_plan::DevPlan;
pub use dev_ready::DevReady;
pub use dev_stack::{run, shutdown_signal};
pub use docker_probe::docker_available;
pub use plan_error::PlanError;
pub use process_spec::ProcessSpec;
pub use readiness::{wait_for_broker, wait_for_tcp};
pub use running_process::{RunningProcess, first_exit, spawn};
pub use worker_kind::WorkerKind;
