//! Entity-mode checks through an in-process broker and the memory worker.
//!
//! Entity mode is broker-side state, so the provider-subject suite cannot see it. See `support/entity.rs` for what
//! runs and why.

mod support;

use std::error::Error;

use support::entity::run_entity_checks;
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_broker::entity_store::SandboxEntityMode;
use trogon_sandbox_memory_worker::{MemoryWorker, MemoryWorkerConfig};

async fn run(mode: SandboxEntityMode) -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let worker = MemoryWorker::start(client.clone(), MemoryWorkerConfig::default()).await?;
    let failures = run_entity_checks(&worker, client, mode, None).await?;
    worker.shutdown().await;
    assert!(
        failures.is_empty(),
        "entity checks failed:\n  {}",
        failures.join("\n  ")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn shadow_mode_mirrors_named_leases_without_changing_the_worker_path() -> Result<(), Box<dyn Error>> {
    run(SandboxEntityMode::Shadow).await
}

#[tokio::test(flavor = "multi_thread")]
async fn on_mode_makes_the_entity_the_record_of_named_leases() -> Result<(), Box<dyn Error>> {
    run(SandboxEntityMode::On).await
}
