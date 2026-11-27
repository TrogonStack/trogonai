use clap::Parser;
use trogon_sandbox_ws_gateway::{GatewayArgs, GatewayError, run, shutdown_signal};

#[tokio::main]
async fn main() -> Result<(), GatewayError> {
    run(GatewayArgs::parse(), shutdown_signal()).await
}
