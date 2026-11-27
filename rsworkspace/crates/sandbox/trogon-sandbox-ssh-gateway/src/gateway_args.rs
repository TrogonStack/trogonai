use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::path::PathBuf;

use clap::Parser;
use trogon_sandbox_auth::{ApiKeyBucket, DEFAULT_BUCKET};
use trogon_sandbox_contract::{PullSecretId, TenantId};
use trogon_sandbox_observability::ObservabilityArgs;

use crate::DEFAULT_HOST_KEY_PATH;

/// Command line flags of the gateway binary.
#[derive(Parser, Debug)]
#[command(name = "trogon-sandbox-ssh-gateway")]
#[command(about = "SSH gateway exposing per-lease ephemeral sessions tunneled over NATS")]
pub struct GatewayArgs {
    #[arg(long, default_value = "0.0.0.0:2222")]
    pub listen: SocketAddr,

    #[arg(long, env = "SANDBOX_API_KEY_BUCKET", default_value = DEFAULT_BUCKET)]
    pub api_key_bucket: ApiKeyBucket,

    #[arg(long, env = "NATS_URL")]
    pub nats_url: String,

    /// Where the credential broker listens, when it is not on the NATS server of `--nats-url`.
    #[arg(long, env = "SANDBOX_CREDENTIAL_BROKER_URL")]
    pub credential_broker_url: Option<String>,

    #[arg(long, env = "SANDBOX_SSH_PULL_SECRET_ID", default_value = "ssh-gateway")]
    pub ssh_pull_secret_id: PullSecretId,

    #[arg(long, env = "SANDBOX_SSH_GATEWAY_TENANT", default_value = "platform")]
    pub ssh_gateway_tenant: TenantId,

    #[arg(long, default_value = DEFAULT_HOST_KEY_PATH)]
    pub host_key_path: PathBuf,

    #[arg(long, default_value = "100")]
    pub max_connections: NonZeroUsize,

    #[command(flatten)]
    pub observability: ObservabilityArgs,
}
