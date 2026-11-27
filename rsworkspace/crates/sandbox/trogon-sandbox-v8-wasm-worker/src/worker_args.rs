use std::path::PathBuf;

use clap::Parser;
use trogon_sandbox_observability::ObservabilityArgs;

/// Command line and environment configuration of the worker.
#[derive(Parser, Debug)]
#[command(
    version,
    about = "Sandbox V8/Wasm worker: runs untrusted JS in a V8 isolate or Wasm in a Wasmtime sandbox, with no hardware virtualization"
)]
pub struct WorkerArgs {
    /// Journal for the tenant and attempt each lease belongs to, so the dispatch fence survives a
    /// restart. Unset keeps it in memory.
    #[arg(long, env = "SANDBOX_DISPATCH_FENCE_FILE")]
    pub dispatch_fence_file: Option<PathBuf>,

    /// NATS server the worker serves leases from.
    #[arg(long, env = "SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    pub nats_url: String,

    /// How many leases the worker advertises it can hold at once.
    #[arg(long, env = "SANDBOX_MAX_CONCURRENT_LEASES", default_value_t = 8)]
    pub max_concurrent_leases: u32,

    #[command(flatten)]
    pub observability: ObservabilityArgs,
}
