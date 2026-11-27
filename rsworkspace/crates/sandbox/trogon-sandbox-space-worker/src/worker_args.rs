use std::path::PathBuf;

use clap::Parser;

/// The command line and environment of the SPACE worker.
#[derive(Parser)]
#[command(about = "Serves sandbox leases as SPACE Firecracker microVMs")]
pub struct WorkerArgs {
    /// NATS server the worker connects to.
    #[arg(long, env = "SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    pub nats_url: String,
    /// Control-plane origin that serves `/v1/...`.
    #[arg(long, env = "SANDBOX_SPACE_API_URL")]
    pub endpoint: url::Url,
    /// Data-plane origin that serves command event streams. Event URLs SPACE returns must
    /// resolve to this origin.
    #[arg(long, env = "SANDBOX_SPACE_DATA_PLANE_URL")]
    pub data_plane_url: url::Url,
    /// Management token, normally supplied through the `SPACE_API_TOKEN` variable.
    #[arg(long, env = "SPACE_API_TOKEN", hide_env_values = true)]
    pub token: String,
    /// SPACE template every lease is created from.
    #[arg(long, env = "SANDBOX_SPACE_TEMPLATE", default_value = "seed:latest")]
    pub template: String,
    /// Directory holding the worker lock and its dispatch ledger.
    #[arg(long, env = "SANDBOX_SPACE_STATE_DIR", default_value = "/var/lib/sandbox-space")]
    pub state_dir: PathBuf,
    /// How many leases the worker holds at once.
    #[arg(long, default_value_t = 4)]
    pub max_leases: u32,
    /// Seconds a create may take until the sandbox answers process queries.
    #[arg(long, default_value_t = 180)]
    pub boot_timeout_seconds: u64,
    /// Seconds pause, suspend, wake and delete may take.
    #[arg(long, default_value_t = 120)]
    pub operation_timeout_seconds: u64,
    /// The worker ID the durable state must carry; unset accepts whatever it carries.
    #[arg(long, env = "SANDBOX_WORKER_ID")]
    pub worker_id: Option<String>,
}
