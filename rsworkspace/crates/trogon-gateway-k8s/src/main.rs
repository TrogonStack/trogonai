use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use kube::Client;
use tracing_subscriber::EnvFilter;

use trogon_gateway_k8s::controller::{
    run_gateway_controller, run_http_route_controller, run_mcp_gateway_config_controller,
    ControllerContext,
};
use trogon_gateway_k8s::health;
use trogon_gateway_k8s::nats::open_default_config_kv;

#[derive(Debug, Parser)]
#[command(
    name = "trogon-gateway-k8s",
    about = "Project Gateway API and Trogon MCP CRDs into mcp-gateway-config NATS KV"
)]
struct Args {
    /// Comma-separated NATS server URLs.
    #[arg(long, env = "NATS_URL", default_value = "nats://127.0.0.1:4222")]
    nats_url: String,

    /// Path to NATS `.creds` file (optional).
    #[arg(long, env = "NATS_CREDS")]
    nats_creds: Option<String>,

    /// Limit watches to a single namespace (recommended per tenant).
    #[arg(long, env = "MCP_GATEWAY_CONTROLLER_WATCH_NAMESPACE")]
    watch_namespace: Option<String>,

    /// HTTP bind address for `/healthz`.
    #[arg(long, default_value = "0.0.0.0:8080")]
    health_addr: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let health_addr: SocketAddr = args.health_addr.parse()?;

    tokio::spawn(async move {
        if let Err(error) = health::serve_healthz(health_addr).await {
            tracing::error!(%error, "health server failed");
        }
    });

    let client = Client::try_default().await?;
    let kv_store =
        open_default_config_kv(&args.nats_url, args.nats_creds.as_deref()).await?;
    let kv: Arc<dyn trogon_gateway_k8s::nats::ConfigKv> = Arc::new(kv_store);
    let ctx = Arc::new(ControllerContext {
        client: client.clone(),
        kv,
    });

    let namespace = args.watch_namespace.clone();
    tokio::try_join!(
        run_mcp_gateway_config_controller(Arc::clone(&ctx), namespace.clone()),
        run_gateway_controller(Arc::clone(&ctx), namespace.clone()),
        run_http_route_controller(ctx, namespace),
    )?;

    Ok(())
}
