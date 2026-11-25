use super::*;
use trogon_egress_proxy::{Readiness, ReadinessStatus};

#[path = "readiness_endpoint_tests.rs"]
mod readiness_endpoint_tests;

async fn status_eventually(readiness: &Readiness, expected: ReadinessStatus) -> TestResult<()> {
    let result = tokio::time::timeout(WAIT, async {
        loop {
            if readiness.probe().await == expected {
                return;
            }
            tokio::time::sleep(TICK).await;
        }
    })
    .await;
    assert!(
        result.is_ok(),
        "expected {expected:?}, observed {:?}",
        readiness.probe().await
    );
    Ok(())
}

#[tokio::test]
async fn readiness_follows_real_workload_authority_and_fresh_audit_acknowledgements() -> TestResult<()> {
    let harness = WorkloadHarness::start("").await?;
    status_eventually(&harness.readiness, ReadinessStatus::Ready).await?;
    let first = harness
        .audit_stream
        .get_last_raw_message_by_subject("platform.v1.audit.integrity.egress_proxy.readiness_probe")
        .await?;
    assert_eq!(harness.readiness.probe().await, ReadinessStatus::Ready);
    let second = harness
        .audit_stream
        .get_last_raw_message_by_subject("platform.v1.audit.integrity.egress_proxy.readiness_probe")
        .await?;
    assert!(
        second.sequence > first.sequence,
        "each probe requires a new acknowledged record"
    );
    harness.workload_api.permission_denied();
    status_eventually(&harness.readiness, ReadinessStatus::IdentityUnavailable).await?;
    harness.stop().await
}

#[tokio::test]
async fn readiness_withdraws_for_live_policy_and_governed_registration_loss() -> TestResult<()> {
    let harness = WorkloadHarness::start("").await?;
    status_eventually(&harness.readiness, ReadinessStatus::Ready).await?;
    harness.policies.delete(&harness.policy_key).await?;
    status_eventually(&harness.readiness, ReadinessStatus::PolicyUnavailable).await?;
    harness
        .policies
        .put(&harness.policy_key, document(2, "")?.into())
        .await?;
    status_eventually(&harness.readiness, ReadinessStatus::Ready).await?;
    harness.registry.delete(&harness.registration_key).await?;
    status_eventually(&harness.readiness, ReadinessStatus::IdentityUnavailable).await?;
    harness.stop().await
}

#[tokio::test]
async fn cancelling_the_runtime_withdraws_readiness_before_background_drain() -> TestResult<()> {
    let harness = WorkloadHarness::start("").await?;
    status_eventually(&harness.readiness, ReadinessStatus::Ready).await?;
    let readiness = harness.readiness.clone();
    harness.shutdown.cancel();
    assert_eq!(readiness.probe().await, ReadinessStatus::Stopped);
    harness.stop().await?;
    assert_eq!(readiness.probe().await, ReadinessStatus::Stopped);
    Ok(())
}
