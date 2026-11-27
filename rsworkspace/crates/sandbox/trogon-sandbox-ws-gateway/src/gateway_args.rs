use std::net::SocketAddr;

use clap::Parser;
use trogon_sandbox_auth::{ApiKeyBucket, DEFAULT_BUCKET};
use trogon_sandbox_observability::ObservabilityArgs;

/// Command line flags of the gateway binary.
#[derive(Parser, Debug)]
#[command(name = "trogon-sandbox-ws-gateway")]
#[command(about = "WebSocket gateway bridging browser clients to the sandbox NATS service")]
pub struct GatewayArgs {
    #[arg(long, default_value = "127.0.0.1:8443")]
    pub listen: SocketAddr,

    #[arg(long, env = "SANDBOX_API_KEY_BUCKET", default_value = DEFAULT_BUCKET)]
    pub api_key_bucket: ApiKeyBucket,

    #[arg(long, env = "NATS_URL")]
    pub nats_url: String,

    /// NATS server that holds the API key bucket, when it is not the one the sandbox service runs on.
    #[arg(long, env = "SANDBOX_API_KEY_STORE_URL")]
    pub api_key_store_url: Option<String>,

    #[arg(long, env = "SANDBOX_WS_CORS_ORIGIN")]
    pub cors_origin: Vec<String>,

    #[arg(long, default_value_t = 3600)]
    pub session_ttl_seconds: u64,

    #[arg(long, env = "SANDBOX_ACTIVE_LEASE_BUCKET", default_value = "sandbox-active-leases")]
    pub active_lease_bucket: String,

    /// Public base URL of this gateway (for example `https://sandbox.example.com`).
    /// Required when preview signing keys are configured.
    #[arg(long, env = "SANDBOX_PREVIEW_PUBLIC_URL")]
    pub preview_public_url: Option<String>,

    #[arg(long, env = "SANDBOX_PREVIEW_TTL_SECONDS", default_value_t = 900)]
    pub preview_ttl_seconds: u64,

    #[command(flatten)]
    pub observability: ObservabilityArgs,
}
