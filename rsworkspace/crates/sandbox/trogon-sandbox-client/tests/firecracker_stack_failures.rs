//! Failure injection against the same stack as `firecracker_stack_smoke`: the VMM killed or frozen mid-lease, the
//! guest agent frozen, and the worker killed with leases live.
//!
//! These kill host processes by pid, so they need `SANDBOX_FIRECRACKER_STATE_DIR` (to find each lease's VMM) and, for
//! the restart case, `SANDBOX_SMOKE_FIRECRACKER_WORKER_PIDFILE` with something that restarts the worker.
//! `devops/docker/compose/sandbox/smoke-firecracker.sh` sets both and runs this file one test at a time, since
//! killing the worker takes every lease it holds with it.

mod common;
mod firecracker_support;
mod stack_support;

use std::time::{Duration, Instant};

use common::TestResult;
use firecracker_support::{
    Fallible, assert_host_resources_released, detached, guest_stdout, lease_request, sh_within, signal, spec_parts,
    vmm_pid, worker_pid,
};
use trogon_sandbox_client::{ClientError, SandboxClient};
use trogon_sandbox_contract::{LeaseGranted, LeaseId, LeaseReleasedReason, ProviderKind, StdinMode};

const TENANT: &str = "failures-tenant";

fn request() -> Fallible<trogon_sandbox_contract::LeaseRequest> {
    lease_request(TENANT, spec_parts(30, 256)?, 300, "failures")
}

/// Retries while a restarted worker is still coming up and nothing answers the dispatch.
async fn lease(client: &SandboxClient) -> Fallible<LeaseGranted> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match client.lease(&request()?, firecracker_support::LEASE_TIMEOUT).await {
            Ok(granted) => {
                assert_eq!(granted.provider, ProviderKind::Firecracker);
                return Ok(granted);
            }
            Err(error) if Instant::now() >= deadline => {
                return Err(format!("no lease was granted: {error:?}").into());
            }
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Kills or freezes something while already listening for the lease's release, since the release is a plain publish
/// that a later subscriber would miss.
async fn released_after(
    client: &SandboxClient,
    lease_id: &LeaseId,
    within: Duration,
    act: impl FnOnce() -> Fallible<()>,
) -> Fallible<LeaseReleasedReason> {
    let (released, acted) = tokio::join!(client.wait_released_with_reason(lease_id, within), async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        act()
    });
    acted?;
    Ok(released?)
}

async fn assert_denied_fast(client: &SandboxClient, lease_id: &LeaseId) -> Fallible<()> {
    let started = Instant::now();
    let result = sh_within(client, lease_id, "true", 5).await;
    assert!(
        matches!(result, Err(ClientError::Denied(_))),
        "a command on a lost lease was not denied: {result:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the denial took {:?}",
        started.elapsed()
    );
    Ok(())
}

/// Cancels the lease and checks its host resources are gone, reporting `outcome` ahead of a cleanup failure.
async fn finish(client: &SandboxClient, lease_id: &LeaseId, outcome: Fallible<()>) -> Fallible<()> {
    let cancelled = client.cancel(lease_id).await;
    let released = assert_host_resources_released(lease_id).await;
    outcome?;
    cancelled?;
    released
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a supervised firecracker worker; run devops/docker/compose/sandbox/smoke-firecracker.sh"]
async fn a_vmm_killed_mid_lease_releases_the_lease_and_its_host_resources() -> TestResult {
    let client = &stack_support::connect().await?;
    let granted = lease(client).await?;
    let lease_id = &granted.lease_id;
    guest_stdout(client, lease_id, "true").await?;
    let pid = vmm_pid(lease_id)?;

    let started = Instant::now();
    let reason = released_after(client, lease_id, Duration::from_secs(15), || signal(pid, "KILL")).await?;
    println!("lease released {:?} after the VMM was killed", started.elapsed());
    assert_eq!(reason, LeaseReleasedReason::Failed);

    assert_denied_fast(client, lease_id).await?;
    assert_host_resources_released(lease_id).await
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a supervised firecracker worker; run devops/docker/compose/sandbox/smoke-firecracker.sh"]
async fn a_frozen_vmm_fails_calls_within_their_timeout_and_recovers_when_thawed() -> TestResult {
    let client = &stack_support::connect().await?;
    let granted = lease(client).await?;
    let lease_id = &granted.lease_id;

    let outcome: Fallible<()> = async {
        guest_stdout(client, lease_id, "echo before > /tmp/mark").await?;
        let pid = vmm_pid(lease_id)?;

        signal(pid, "STOP")?;
        let started = Instant::now();
        let frozen = sh_within(client, lease_id, "true", 1).await;
        let waited = started.elapsed();
        signal(pid, "CONT")?;

        assert!(frozen.is_err(), "a frozen VM answered: {frozen:?}");
        assert!(waited < Duration::from_secs(15), "the call took {waited:?}");
        assert_eq!(guest_stdout(client, lease_id, "cat /tmp/mark").await?.trim(), "before");
        Ok(())
    }
    .await;
    finish(client, lease_id, outcome).await
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a supervised firecracker worker; run devops/docker/compose/sandbox/smoke-firecracker.sh"]
async fn a_frozen_guest_agent_fails_calls_within_their_timeout_and_recovers_when_thawed() -> TestResult {
    let client = &stack_support::connect().await?;
    let granted = lease(client).await?;
    let lease_id = &granted.lease_id;

    let outcome: Fallible<()> = async {
        guest_stdout(client, lease_id, "true").await?;

        // The freezer runs as a child of the agent, which a stopped agent does not stop.
        detached(
            client,
            lease_id,
            "pid=$(systemctl show --value -p MainPID sandbox-actor-agent); \
             sleep 1; kill -STOP $pid; sleep 10; kill -CONT $pid",
            StdinMode::Closed,
        )
        .await?;
        tokio::time::sleep(Duration::from_secs(2)).await;

        let started = Instant::now();
        let frozen = sh_within(client, lease_id, "true", 1).await;
        let waited = started.elapsed();
        assert!(frozen.is_err(), "a frozen agent answered: {frozen:?}");
        assert!(waited < Duration::from_secs(15), "the call took {waited:?}");

        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(output) = sh_within(client, lease_id, "echo thawed", 5).await
                && output.exit_code == 0
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("the agent never came back".into());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    .await;
    finish(client, lease_id, outcome).await
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a supervised firecracker worker; run devops/docker/compose/sandbox/smoke-firecracker.sh"]
async fn a_worker_killed_with_live_leases_reclaims_them_when_it_restarts() -> TestResult {
    let client = &stack_support::connect().await?;
    let granted = lease(client).await?;
    let lease_id = &granted.lease_id;
    guest_stdout(client, lease_id, "true").await?;
    let worker = worker_pid()?;

    let started = Instant::now();
    let reason = released_after(client, lease_id, Duration::from_secs(60), || signal(worker, "KILL")).await?;
    println!("lease released {:?} after the worker was killed", started.elapsed());
    assert_eq!(reason, LeaseReleasedReason::Failed);
    assert_ne!(worker_pid()?, worker, "the worker was not restarted");
    assert_host_resources_released(lease_id).await?;

    let fresh = lease(client).await?;
    let outcome: Fallible<()> = async {
        assert_eq!(guest_stdout(client, &fresh.lease_id, "echo ok").await?.trim(), "ok");
        Ok(())
    }
    .await;
    finish(client, &fresh.lease_id, outcome).await
}
