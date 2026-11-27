use clap::Parser;
use trogon_sandbox_memory_worker::{MemoryWorkerArgs, RunError, run};

#[tokio::main]
async fn main() -> Result<(), RunError> {
    run(MemoryWorkerArgs::parse(), async {
        // Ignoring the error stops the worker the same as the signal would.
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
}
