//! Lease-to-ready timings against the same stack as `firecracker_stack_smoke`, one lease at a time and four at once.
//! `devops/docker/compose/sandbox/smoke-firecracker.sh` runs this file on its own, after the other lanes, so nothing
//! else competes for the host while it measures.

mod common;
mod firecracker_support;
mod stack_support;

use std::time::{Duration, Instant};

use common::TestResult;
use firecracker_support::{Fallible, LEASE_TIMEOUT, lease_request, spec_parts};
use trogon_sandbox_client::SandboxClient;
use trogon_sandbox_contract::LeaseGranted;

const ROUNDS: usize = 5;
const CONCURRENT: usize = 4;

async fn timed_lease(client: &SandboxClient) -> Fallible<(LeaseGranted, Duration)> {
    let request = lease_request("timings-tenant", spec_parts(30, 256)?, 120, "timings")?;
    let started = Instant::now();
    let granted = client.lease(&request, LEASE_TIMEOUT).await?;
    Ok((granted, started.elapsed()))
}

async fn release(client: &SandboxClient, grants: &[LeaseGranted]) -> Fallible<()> {
    for granted in grants {
        client.cancel(&granted.lease_id).await?;
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    Ok(())
}

fn summary(samples: &mut [Duration]) -> String {
    samples.sort();
    format!(
        "min {:?}, median {:?}, max {:?}",
        samples[0],
        samples[samples.len() / 2],
        samples[samples.len() - 1]
    )
}

#[tokio::test]
#[ignore = "requires /dev/kvm, the broker and a firecracker worker; run devops/docker/compose/sandbox/smoke-firecracker.sh"]
async fn lease_to_ready_one_at_a_time_and_four_at_once() -> TestResult {
    let client = &stack_support::connect().await?;

    let mut single = Vec::new();
    for _ in 0..ROUNDS {
        let (granted, elapsed) = timed_lease(client).await?;
        single.push(elapsed);
        release(client, &[granted]).await?;
    }

    let mut slowest = Vec::new();
    for _ in 0..ROUNDS {
        let started = Instant::now();
        let results = futures::future::join_all((0..CONCURRENT).map(|_| timed_lease(client))).await;
        slowest.push(started.elapsed());
        let mut grants = Vec::new();
        let mut failure = None;
        for result in results {
            match result {
                Ok((granted, _)) => grants.push(granted),
                Err(error) => failure = Some(error),
            }
        }
        release(client, &grants).await?;
        if let Some(error) = failure {
            return Err(error);
        }
    }

    println!("lease to ready, one at a time: {}", summary(&mut single));
    println!(
        "lease to ready, {CONCURRENT} at once (slowest of each batch): {}",
        summary(&mut slowest)
    );
    Ok(())
}
