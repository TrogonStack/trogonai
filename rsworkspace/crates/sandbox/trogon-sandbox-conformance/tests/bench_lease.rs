//! The bench lease seeds files and reads them back through `cat` on the memory worker, the round trips the
//! `memory_worker` bench times.

mod support;

use std::error::Error;

use support::memory::{drained, memory_target};
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_conformance::BenchLease;
use trogon_sandbox_memory_worker::{MemoryWorker, MemoryWorkerConfig};

#[tokio::test(flavor = "multi_thread")]
async fn a_seeded_file_reads_back_at_its_length() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let worker = MemoryWorker::start(client.clone(), MemoryWorkerConfig::default()).await?;
    let target = memory_target(client);

    let lease = BenchLease::acquire(&target).await?;
    for len in [0, 1024, 65_536] {
        let path = format!("/work/bench-{len}.bin");
        lease.seed_file(&path, len).await?;
        assert_eq!(lease.cat(&path).await?, len, "{path}");
    }
    let missing = lease.cat("/work/absent.bin").await;
    assert!(missing.is_err(), "cat of a missing file must fail: {missing:?}");
    lease.release().await;

    assert_eq!(drained(&worker).await, 0, "the release cancels the lease");
    worker.shutdown().await;
    Ok(())
}
