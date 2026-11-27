//! Forking a running Firecracker lease: the broker checkpoints the source without stopping it and restores the
//! checkpoint into a new lease. Run it with `devops/docker/compose/sandbox/smoke-firecracker.sh`, whose worker keeps
//! snapshots.

mod common;
mod firecracker_support;
mod stack_support;

use std::time::{Duration, Instant};

use common::TestResult;
use firecracker_support::{
    Fallible, assert_host_resources_released, guest_sh, guest_stdout, lease_request, spec_parts, with_lease,
};
use trogon_sandbox_client::SandboxClient;
use trogon_sandbox_contract::{LeaseId, TenantId};

const ROUNDS: usize = 3;
const ACTOR_ID_PATH: &str = "/run/ate/actor-id";
const TENANT: &str = "snapshots-tenant";

async fn ticks(client: &SandboxClient, lease_id: &LeaseId) -> Fallible<u64> {
    Ok(guest_stdout(client, lease_id, "cat /workspace/tick")
        .await?
        .trim()
        .parse()?)
}

async fn advances(client: &SandboxClient, lease_id: &LeaseId) -> Fallible<()> {
    let before = ticks(client, lease_id).await?;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let after = ticks(client, lease_id).await?;
    assert!(after > before, "the loop stopped: {before} -> {after}");
    Ok(())
}

/// The source keeps running and the fork carries its files and processes, but answers as its own lease.
async fn fork_once(client: &SandboxClient, source: &LeaseId, pid: &str) -> Fallible<Duration> {
    let started = Instant::now();
    let fork = client
        .fork(
            TenantId::new(TENANT)?,
            source,
            120,
            stack_support::unique("snapshots-fork"),
        )
        .await?;
    let elapsed = started.elapsed();
    let fork_id = fork.lease_id.clone();

    let outcome: Fallible<()> = async {
        assert_eq!(
            guest_stdout(client, &fork_id, "cat /workspace/marker").await?.trim(),
            "written before the fork"
        );
        guest_stdout(client, &fork_id, &format!("kill -0 {pid}")).await?;
        advances(client, &fork_id).await?;
        assert_eq!(
            guest_stdout(client, &fork_id, &format!("cat {ACTOR_ID_PATH}"))
                .await?
                .trim(),
            fork_id.as_str()
        );

        guest_stdout(client, &fork_id, "echo fork-only > /workspace/fork").await?;
        let source_sees = guest_sh(client, source, "test -e /workspace/fork").await?;
        assert_ne!(source_sees.exit_code, 0, "the fork wrote into the source");
        advances(client, source).await
    }
    .await;
    let cancelled = client.cancel(&fork_id).await;
    let released = assert_host_resources_released(&fork_id).await;
    outcome?;
    cancelled?;
    released?;
    Ok(elapsed)
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker that keeps snapshots; run devops/docker/compose/sandbox/smoke-firecracker.sh"]
async fn a_forked_lease_keeps_the_files_and_processes_of_its_source() -> TestResult {
    let client = &stack_support::connect().await?;
    let request = lease_request(TENANT, spec_parts(60, 256)?, 300, "snapshots-source")?;

    with_lease(client, request, |source| async move {
        let source = &source.lease_id;
        guest_stdout(client, source, "echo 'written before the fork' > /workspace/marker").await?;
        let pid = guest_stdout(
            client,
            source,
            "i=0; (while true; do i=$((i+1)); echo $i > /workspace/tick; sleep 0.2; done) \
             </dev/null >/dev/null 2>&1 & echo $!",
        )
        .await?;
        let pid = pid.trim();
        advances(client, source).await?;

        let mut timings = Vec::new();
        for _ in 0..ROUNDS {
            timings.push(fork_once(client, source, pid).await?);
        }
        timings.sort();
        println!(
            "fork (checkpoint of the running source and restore) to ready: min {:?}, median {:?}, max {:?}",
            timings[0],
            timings[timings.len() / 2],
            timings[timings.len() - 1]
        );
        Ok(())
    })
    .await
}
