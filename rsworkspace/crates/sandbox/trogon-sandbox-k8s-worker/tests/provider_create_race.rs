mod support;

use support::*;
use trogon_sandbox_k8s_worker::K8sWorkerError;
use trogon_sandbox_protocol::{Provider, ProviderError};

#[tokio::test]
async fn two_concurrent_creates_for_one_lease_launch_one_sandbox() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-race");
    let request = lease_request()?;
    let (first, second) = tokio::join!(
        world.provider.lease(&request, &lease),
        world.provider.lease(&request, &lease)
    );
    let outcomes = [first, second];
    let granted = outcomes.iter().filter(|outcome| outcome.is_ok()).count();
    let refused = outcomes
        .iter()
        .filter(|outcome| {
            matches!(
                outcome,
                Err(ProviderError::Backend(source))
                    if matches!(source.downcast_ref(), Some(K8sWorkerError::LeaseAlreadyHeld))
            )
        })
        .count();
    assert_eq!((granted, refused), (1, 1));
    assert_eq!(world.cluster.state().jobs_created, 1);
    assert_eq!(world.provider.active_lease_count().await, 1);
    Ok(())
}
