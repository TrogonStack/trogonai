use std::path::PathBuf;

use clap::Parser;
use trogon_sandbox_observability::ObservabilityArgs;
use url::Url;

use crate::api::{DEFAULT_API_URL, DEFAULT_DOMAIN};
use crate::constants::{DEFAULT_HTTP_TIMEOUT_SECS, DEFAULT_MAX_CONCURRENT_LEASES, DEFAULT_NATS_URL, DEFAULT_TEMPLATE};

/// Everything the worker binary is configured with.
#[derive(Parser, Debug)]
#[command(version, about = "Sandbox E2B worker: serves sandbox lease requests via the E2B API")]
pub struct WorkerArgs {
    /// Retains the worker identity and exact assignment receipts across restarts.
    #[arg(long, env = "SANDBOX_E2B_DISPATCH_LEDGER_FILE")]
    pub dispatch_ledger_file: Option<PathBuf>,

    /// Journal for the tenant and attempt each lease belongs to, so the dispatch fence survives a restart. Unset keeps
    /// it in memory.
    #[arg(long, env = "SANDBOX_DISPATCH_FENCE_FILE")]
    pub dispatch_fence_file: Option<PathBuf>,

    #[arg(long, env = "SANDBOX_NATS_URL", default_value = DEFAULT_NATS_URL)]
    pub nats_url: String,

    /// The E2B API key. Read from the environment; never printed.
    #[arg(long, env = "E2B_API_KEY", hide_env_values = true)]
    pub api_key: String,

    #[arg(long, env = "E2B_API_URL", default_value = DEFAULT_API_URL)]
    pub api_url: Url,

    /// The domain sandboxes are served under (`<port>-<sandbox>.<domain>`).
    #[arg(long, env = "E2B_DOMAIN", default_value = DEFAULT_DOMAIN)]
    pub domain: String,

    /// Overrides the sandbox envd address for a self-hosted endpoint.
    #[arg(long, env = "SANDBOX_E2B_ENVD_URL")]
    pub envd_url: Option<Url>,

    /// Template used when a lease names no image.
    #[arg(long, env = "SANDBOX_E2B_TEMPLATE", default_value = DEFAULT_TEMPLATE)]
    pub template: String,

    #[arg(long, env = "SANDBOX_MAX_CONCURRENT_LEASES", default_value_t = DEFAULT_MAX_CONCURRENT_LEASES)]
    pub max_concurrent_leases: u32,

    /// Timeout in seconds applied to every E2B request. A foreground command holds one request open, so it also
    /// bounds how long a command can run.
    #[arg(
        long,
        env = "SANDBOX_E2B_HTTP_TIMEOUT_SECS",
        default_value_t = DEFAULT_HTTP_TIMEOUT_SECS,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    pub http_timeout_secs: u64,

    /// Regions this worker offers, comma separated. E2B places sandboxes itself, so the default is none and a lease
    /// naming a region never reaches this worker.
    #[arg(long, env = "SANDBOX_E2B_REGIONS", value_delimiter = ',')]
    pub regions: Vec<String>,

    #[command(flatten)]
    pub observability: ObservabilityArgs,
}

#[cfg(test)]
mod tests;
