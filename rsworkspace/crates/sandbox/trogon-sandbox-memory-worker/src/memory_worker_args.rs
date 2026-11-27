use std::path::PathBuf;

use clap::Parser;

/// Command line of the memory worker binary.
#[derive(Parser, Debug)]
#[command(
    version,
    about = "In-memory sandbox worker for tests: scripted commands over a file map, no containers"
)]
pub struct MemoryWorkerArgs {
    /// Journal for the tenant and attempt each lease belongs to, so the
    /// dispatch fence survives a restart. Unset keeps it in memory.
    #[arg(long, env = "SANDBOX_DISPATCH_FENCE_FILE")]
    pub dispatch_fence_file: Option<PathBuf>,

    /// The NATS server to serve the provider subjects on.
    #[arg(long, env = "SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    pub nats_url: String,

    /// The most leases the worker advertises it can hold.
    #[arg(long, env = "SANDBOX_MAX_CONCURRENT_LEASES", default_value_t = 64)]
    pub max_concurrent_leases: u32,

    /// Regions this worker claims to place sandboxes in, comma separated.
    #[arg(long, env = "SANDBOX_MEMORY_REGIONS", value_delimiter = ',')]
    pub regions: Vec<String>,
}
