use std::process::ExitCode;

use clap::Parser;
use trogon_sandbox_hyperlight_worker::{UnsupportedHypervisor, WorkerArgs, serve};

#[tokio::main]
async fn main() -> ExitCode {
    serve(WorkerArgs::parse(), UnsupportedHypervisor).await
}
