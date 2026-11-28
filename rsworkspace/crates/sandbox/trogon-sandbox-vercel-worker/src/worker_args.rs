use std::path::PathBuf;

use clap::Parser;
use trogon_sandbox_observability::ObservabilityArgs;

use crate::constants::{DEFAULT_API_URL, DEFAULT_MAX_VCPUS};
use crate::{VercelEndpoint, VercelPlan, VercelProjectId, VercelTeamId, VercelToken};

/// Everything the worker binary is configured with.
#[derive(Parser, Debug)]
#[command(
    version,
    about = "Sandbox Vercel worker: serves sandbox lease requests via the Vercel API"
)]
pub struct WorkerArgs {
    /// Journal for the tenant and attempt each lease belongs to, so the dispatch fence survives a restart. Unset keeps
    /// it in memory.
    #[arg(long, env = "SANDBOX_DISPATCH_FENCE_FILE")]
    pub dispatch_fence_file: Option<PathBuf>,

    /// Keeps each targeted dispatch attempt and its receipt, so a repeat gets the first sandbox and a cleanup is
    /// confirmed only once the sandbox is gone. Setting it makes the worker announce targeted dispatch and take its
    /// worker id from the ledger. Unset, the worker serves only the shared lease subject.
    #[arg(long, env = "SANDBOX_VERCEL_DISPATCH_LEDGER_FILE")]
    pub dispatch_ledger_file: Option<PathBuf>,

    #[arg(long, env = "SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    pub nats_url: String,

    /// The Vercel access token. Read from the environment; never printed.
    #[arg(long, env = "VERCEL_TOKEN", hide_env_values = true)]
    pub token: VercelToken,

    #[arg(long, env = "VERCEL_TEAM_ID")]
    pub team_id: VercelTeamId,

    #[arg(long, env = "VERCEL_PROJECT_ID")]
    pub project_id: VercelProjectId,

    #[arg(long, env = "SANDBOX_VERCEL_API_URL", default_value = DEFAULT_API_URL)]
    pub api_url: VercelEndpoint,

    /// Runtime used when a lease names no image. One of node26, node24, node22 or python3.13.
    #[arg(long, env = "SANDBOX_VERCEL_RUNTIME", default_value = "node24")]
    pub runtime: String,

    /// The Vercel plan the team is on: hobby, pro or enterprise. It caps how long a lease may run and how many vCPUs
    /// it may ask for.
    #[arg(long, env = "SANDBOX_VERCEL_PLAN", default_value_t = VercelPlan::Pro)]
    pub plan: VercelPlan,

    /// Most vCPUs a lease may ask for. Memory is fixed per vCPU by Vercel.
    #[arg(
        long,
        env = "SANDBOX_VERCEL_MAX_VCPUS",
        default_value_t = DEFAULT_MAX_VCPUS,
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    pub max_vcpus: u32,

    #[arg(long, env = "SANDBOX_MAX_CONCURRENT_LEASES", default_value_t = 4)]
    pub max_concurrent_leases: u32,

    /// Timeout in seconds applied to every Vercel request. A foreground command holds one request open, so it also
    /// bounds how long a command can run.
    #[arg(
        long,
        env = "SANDBOX_VERCEL_HTTP_TIMEOUT_SECS",
        default_value_t = 300,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    pub http_timeout_secs: u64,

    /// Regions this worker offers, comma separated. The default is none, so a lease naming a region never reaches
    /// this worker.
    #[arg(long, env = "SANDBOX_VERCEL_REGIONS", value_delimiter = ',')]
    pub regions: Vec<String>,

    #[command(flatten)]
    pub observability: ObservabilityArgs,
}

#[cfg(test)]
mod tests;
