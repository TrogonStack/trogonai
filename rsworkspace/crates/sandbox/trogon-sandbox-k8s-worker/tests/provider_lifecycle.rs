mod support;

use support::*;
use trogon_sandbox_protocol::Provider;

#[tokio::test]
async fn a_lease_creates_a_job_and_cancel_removes_it() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-one");
    let granted = world.provider.lease(&lease_request()?, &lease).await?;
    assert_eq!(world.cluster.names("jobs").len(), 1);
    assert!(world.provider.holds_lease(&lease).await);
    assert_eq!(granted.sandbox_id.as_str(), world.cluster.names("jobs")[0]);
    world.provider.cancel(&lease).await?;
    assert!(world.cluster.names("jobs").is_empty());
    Ok(())
}
