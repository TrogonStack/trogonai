use clap::Parser;
use trogon_sandbox_dev::{DevArgs, run, shutdown_signal};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = DevArgs::parse();
    run(args, shutdown_signal()?, |ready| println!("{ready}")).await?;
    Ok(())
}
