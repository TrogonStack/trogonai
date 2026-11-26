use std::net::SocketAddr;

use clap::Parser;
use trogon_sandbox_observability::ObservabilityArgs;

use crate::WsGatewayUrl;

/// Command line flags of the dashboard binary.
#[derive(Parser, Debug)]
#[command(name = "trogon-sandbox-dashboard")]
#[command(about = "Static single-page dashboard for sandbox observability through the WebSocket gateway")]
pub struct DashboardArgs {
    #[arg(long, default_value = "127.0.0.1:8080")]
    pub listen: SocketAddr,

    #[arg(long, default_value = "ws://127.0.0.1:8443/v1/ws")]
    pub ws_gateway_url: WsGatewayUrl,

    #[command(flatten)]
    pub observability: ObservabilityArgs,
}
