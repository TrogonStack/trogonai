mod cli;
mod run;

use anyhow::Context;
use clap::Parser;
use trogon_sandbox_admin::Console;

use crate::cli::Cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let nats = trogon_sandbox_protocol::nats_connect::connect(&cli.nats_url)
        .await
        .context("connect to NATS")?;
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let mut console = Console::new(&mut stdout, &mut stderr);
    run::run(cli, nats, &mut console).await
}
