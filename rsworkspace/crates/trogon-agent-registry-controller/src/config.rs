use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;

use crate::signer::SignerKind;

#[derive(Debug, Clone, Parser)]
#[command(name = "trogon-agent-registry-controller")]
#[command(about = "Git-sync control plane for the agent registry KV bucket")]
pub struct ControllerConfig {
    #[arg(long, env = "NATS_URL", default_value = "nats://127.0.0.1:4222")]
    pub nats_url: String,

    #[arg(long, env = "TROGON_REGISTRY_AUTOCREATE", default_value_t = false)]
    pub auto_create_bucket: bool,

    #[arg(long, env = "TROGON_REGISTRY_CONTROLLER_REPO_PATH")]
    pub repo_path: PathBuf,

    #[arg(long, env = "TROGON_REGISTRY_CONTROLLER_GIT_REMOTE")]
    pub git_remote: Option<String>,

    #[arg(long, env = "TROGON_REGISTRY_CONTROLLER_GIT_REF", default_value = "main")]
    pub git_ref: String,

    #[arg(long, env = "TROGON_REGISTRY_CONTROLLER_AGENTS_DIR", default_value = "agents")]
    pub agents_dir: PathBuf,

    #[arg(
        long,
        env = "TROGON_REGISTRY_CONTROLLER_SYNC_INTERVAL_S",
        default_value_t = 30
    )]
    pub sync_interval_secs: u64,

    #[arg(long, env = "TROGON_REGISTRY_CONTROLLER_SIGNER_SOURCE", default_value = "file")]
    pub signer_source: SignerKind,

    #[arg(long, env = "TROGON_REGISTRY_CONTROLLER_SIGNER_KEY_PATH")]
    pub signer_key_path: PathBuf,
}

impl ControllerConfig {
    pub fn sync_interval(&self) -> Duration {
        Duration::from_secs(self.sync_interval_secs)
    }
}
