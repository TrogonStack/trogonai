use std::path::PathBuf;

use clap::Parser;
use trogon_sandbox_observability::ObservabilityArgs;

use crate::constants::{DEFAULT_API_URL, DEFAULT_SNAPSHOT_POLL_INTERVAL_SECS, DEFAULT_SNAPSHOT_TIMEOUT_SECS};
use crate::{DaytonaApiKey, DaytonaEndpoint, DaytonaOrganizationId};

/// Everything the worker binary is configured with.
#[derive(Parser, Debug)]
#[command(
    version,
    about = "Sandbox Daytona worker: serves sandbox lease requests via the Daytona API"
)]
pub struct WorkerArgs {
    /// Journal for the tenant and attempt each lease belongs to, so the dispatch fence survives a restart. Unset keeps
    /// it in memory.
    #[arg(long, env = "SANDBOX_DISPATCH_FENCE_FILE")]
    pub dispatch_fence_file: Option<PathBuf>,

    /// Keeps each targeted dispatch attempt and its receipt, so a repeat gets the first sandbox and a cleanup is
    /// confirmed only once the sandbox is gone. Setting it makes the worker announce targeted dispatch and take its
    /// worker id from the ledger. Unset, the worker serves only the shared lease subject.
    #[arg(long, env = "SANDBOX_DAYTONA_DISPATCH_LEDGER_FILE")]
    pub dispatch_ledger_file: Option<PathBuf>,

    #[arg(long, env = "SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    pub nats_url: String,

    /// The Daytona API key. Read from the environment; never printed.
    #[arg(long, env = "DAYTONA_API_KEY", hide_env_values = true)]
    pub api_key: DaytonaApiKey,

    /// The organization the key acts for. Unset uses the key's own.
    #[arg(long, env = "DAYTONA_ORGANIZATION_ID")]
    pub organization_id: Option<DaytonaOrganizationId>,

    #[arg(long, env = "DAYTONA_BASE_URL", default_value = DEFAULT_API_URL)]
    pub api_url: DaytonaEndpoint,

    /// Snapshot a sandbox starts from when a lease names no image.
    #[arg(long, env = "SANDBOX_DAYTONA_SNAPSHOT", default_value = "sandbox-agent:dev")]
    pub snapshot: String,

    #[arg(long, env = "SANDBOX_MAX_CONCURRENT_LEASES", default_value_t = 4)]
    pub max_concurrent_leases: u32,

    /// Timeout in seconds applied to every Daytona request. A foreground command holds one request open, so it also
    /// bounds how long a command can run.
    #[arg(
        long,
        env = "SANDBOX_DAYTONA_HTTP_TIMEOUT_SECS",
        default_value_t = 300,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    pub http_timeout_secs: u64,

    /// Seconds a sandbox may take to start or stop before the lease fails.
    #[arg(
        long,
        env = "SANDBOX_DAYTONA_START_TIMEOUT_SECS",
        default_value_t = 300,
        value_parser = clap::value_parser!(u64).range(1..=3600)
    )]
    pub start_timeout_secs: u64,

    /// Seconds between two looks at a native snapshot that is still building.
    #[arg(
        long,
        env = "SANDBOX_DAYTONA_SNAPSHOT_POLL_INTERVAL_SECS",
        default_value_t = DEFAULT_SNAPSHOT_POLL_INTERVAL_SECS,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    pub snapshot_poll_interval_secs: u64,

    /// Seconds a native snapshot may take to turn active before the capture is reported as not finished.
    #[arg(
        long,
        env = "SANDBOX_DAYTONA_SNAPSHOT_TIMEOUT_SECS",
        default_value_t = DEFAULT_SNAPSHOT_TIMEOUT_SECS,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    pub snapshot_timeout_secs: u64,

    /// Daytona regions this worker can place sandboxes in (the API's `target`), comma separated. Empty means the
    /// worker offers no region choice, so a lease naming a region never reaches it.
    #[arg(long, env = "SANDBOX_DAYTONA_REGIONS", value_delimiter = ',')]
    pub regions: Vec<String>,

    /// A command supervisor artifact the sandboxes run ordinary commands through. Needs its digest.
    #[arg(
        long,
        env = "SANDBOX_DAYTONA_COMMAND_SUPERVISOR",
        requires = "command_supervisor_sha256"
    )]
    pub command_supervisor: Option<PathBuf>,

    /// The lowercase hex SHA-256 the supervisor artifact must have. Needs the artifact.
    #[arg(
        long,
        env = "SANDBOX_DAYTONA_COMMAND_SUPERVISOR_SHA256",
        requires = "command_supervisor"
    )]
    pub command_supervisor_sha256: Option<String>,

    #[command(flatten)]
    pub observability: ObservabilityArgs,
}

#[cfg(test)]
mod tests;
