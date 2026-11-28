use std::path::PathBuf;

use clap::Parser;
use trogon_sandbox_observability::ObservabilityArgs;

use crate::constants::{DEFAULT_GUEST_HEAP_BYTES, DEFAULT_GUEST_INPUT_BUFFER_BYTES, DEFAULT_GUEST_OUTPUT_BUFFER_BYTES};

/// The command line and environment of the Hyperlight worker.
#[derive(Debug, Parser)]
#[command(
    version,
    about = "Serves sandbox leases as Wasm components in hypervisor-isolated guests"
)]
pub struct WorkerArgs {
    /// Journal for the tenant and attempt each lease belongs to, so the dispatch fence survives
    /// a restart. Unset keeps it in memory.
    #[arg(long, env = "SANDBOX_DISPATCH_FENCE_FILE")]
    pub dispatch_fence_file: Option<PathBuf>,
    /// NATS server the worker connects to.
    #[arg(long, env = "SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    pub nats_url: String,
    /// Directory the worker resolves component names under. The `image` field on the wire is a
    /// logical name that resolves to a `.wasm` file here.
    #[arg(long, env = "SANDBOX_HYPERLIGHT_COMPONENT_ROOT")]
    pub component_root: PathBuf,
    /// How many leases the worker holds at once.
    #[arg(long, env = "SANDBOX_MAX_CONCURRENT_LEASES", default_value_t = 8)]
    pub max_concurrent_leases: u32,
    /// How many warm runtimes the worker keeps for the next lease.
    #[arg(long, env = "SANDBOX_MAX_WARM_SANDBOXES", default_value_t = 4)]
    pub max_warm_sandboxes: u32,
    /// Registry a snapshot is pushed to when its target names none. Restores always use the
    /// registry of the artifact reference.
    #[arg(long, env = "SANDBOX_SNAPSHOT_REGISTRY", default_value = "127.0.0.1:5000")]
    pub snapshot_registry: String,
    /// Registries to reach over plain HTTP instead of HTTPS, comma-separated.
    #[arg(
        long,
        env = "SANDBOX_OCI_INSECURE_REGISTRIES",
        value_delimiter = ',',
        default_value = ""
    )]
    pub insecure_registries: Vec<String>,
    /// Bytes of input buffer each guest gets: what the worker hands a guest per call. Hyperlight
    /// raises a smaller value to its own minimum.
    #[arg(long, env = "SANDBOX_HYPERLIGHT_GUEST_INPUT_BUFFER_BYTES", default_value_t = DEFAULT_GUEST_INPUT_BUFFER_BYTES)]
    pub guest_input_buffer_bytes: usize,
    /// Bytes of output buffer each guest gets: the most it can hand back in one return value or
    /// host-call argument.
    #[arg(long, env = "SANDBOX_HYPERLIGHT_GUEST_OUTPUT_BUFFER_BYTES", default_value_t = DEFAULT_GUEST_OUTPUT_BUFFER_BYTES)]
    pub guest_output_buffer_bytes: usize,
    /// Bytes of heap each guest gets; its Wasm linear memory lives there. Hyperlight raises a
    /// smaller value to its own minimum.
    #[arg(long, env = "SANDBOX_HYPERLIGHT_GUEST_HEAP_BYTES", default_value_t = DEFAULT_GUEST_HEAP_BYTES)]
    pub guest_heap_bytes: u64,
    #[command(flatten)]
    pub observability: ObservabilityArgs,
}
