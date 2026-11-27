use std::io::Write as _;

use clap::Parser;
use trogon_sandbox_dev::{DevArgs, run, shutdown_signal};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = DevArgs::parse();
    let mut banner = Ok(());
    run(args, shutdown_signal()?, |ready| {
        banner = writeln!(std::io::stdout(), "{ready}");
    })
    .await?;
    banner?;
    Ok(())
}
