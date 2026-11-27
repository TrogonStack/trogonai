use clap::Parser;
use trogon_sandbox_ssh_gateway::{GatewayArgs, run_gateway, shutdown_signal};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run_gateway(GatewayArgs::parse(), shutdown_signal()).await?;
    Ok(())
}
