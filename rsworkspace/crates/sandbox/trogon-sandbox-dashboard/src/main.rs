use clap::Parser;
use trogon_sandbox_dashboard::{DashboardArgs, run, shutdown_signal};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run(DashboardArgs::parse(), shutdown_signal()).await?;
    Ok(())
}
