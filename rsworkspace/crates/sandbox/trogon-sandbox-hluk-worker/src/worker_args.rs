use std::path::PathBuf;

use clap::Parser;

/// Command line and environment configuration of the worker.
#[derive(Parser, Debug)]
#[command(
    version,
    about = "Sandbox hluk worker (spike): runs each command of a lease in a Hyperlight micro-VM restored by hluk"
)]
pub struct WorkerArgs {
    /// NATS server the worker serves leases from.
    #[arg(long, env = "SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    pub nats_url: String,

    /// The `hluk` binary.
    #[arg(long, env = "SANDBOX_HLUK_BIN", default_value = "hluk")]
    pub hluk_bin: PathBuf,

    /// Directory the worker creates each lease's workspace in.
    #[arg(long, env = "SANDBOX_HLUK_WORK_DIR", default_value = "/tmp/trogon-sandbox-hluk")]
    pub work_dir: PathBuf,

    /// How many leases the worker advertises it can hold at once.
    #[arg(long, env = "SANDBOX_MAX_CONCURRENT_LEASES", default_value_t = 64)]
    pub max_concurrent_leases: u32,
}
