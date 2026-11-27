use std::sync::Arc;

use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    CancelIntent, DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderCleanupRequest,
    ProviderKind, ProviderLease, SandboxId, TenantId, WorkerId,
};
use trogon_sandbox_protocol::AttemptLedger;
use wiremock::MockServer;

use super::tests::{TestResult, build_provider};
use super::{ActiveLease, E2bDispatchError, E2bWorkerProvider, LeaseAccess, LeaseLifetime, PauseOwnership};
use crate::api::TrafficPrivacy;
use crate::envd::EnvdTarget;

fn identity(epoch: Option<LeaseEpoch>) -> Result<LeaseAttemptIdentity, Box<dyn std::error::Error>> {
    Ok(LeaseAttemptIdentity {
        lease_id: LeaseId::new("lease-1"),
        provider: ProviderKind::E2b,
        target: DispatchTarget {
            worker_id: WorkerId::new("worker-1")?,
            attempt_id: DispatchAttemptId::new("attempt-1")?,
            epoch,
        },
    })
}

fn receipt(identity: &LeaseAttemptIdentity) -> Result<ProviderLease, Box<dyn std::error::Error>> {
    Ok(ProviderLease {
        sandbox_id: SandboxId::new("sbx1")?,
        evidence_subject: "evidence".to_owned(),
        worker_id: Some(identity.target.worker_id.clone()),
        attempt_id: Some(identity.target.attempt_id.clone()),
        epoch: identity.target.epoch,
    })
}

#[test]
fn a_receipt_naming_the_assignment_is_accepted() -> TestResult {
    let identity = identity(Some(LeaseEpoch::new(2)?))?;
    assert!(E2bWorkerProvider::verify_receipt_identity(&identity, &receipt(&identity)?).is_ok());
    Ok(())
}

#[test]
fn a_receipt_naming_another_worker_attempt_or_epoch_is_refused() -> TestResult {
    let identity = identity(Some(LeaseEpoch::new(2)?))?;
    let mut other_worker = receipt(&identity)?;
    other_worker.worker_id = Some(WorkerId::new("worker-2")?);
    let mut other_attempt = receipt(&identity)?;
    other_attempt.attempt_id = Some(DispatchAttemptId::new("attempt-2")?);
    let mut other_epoch = receipt(&identity)?;
    other_epoch.epoch = Some(LeaseEpoch::new(3)?);
    for changed in [other_worker, other_attempt, other_epoch] {
        assert!(matches!(
            E2bWorkerProvider::verify_receipt_identity(&identity, &changed),
            Err(E2bDispatchError::ReceiptChangedAssignment)
        ));
    }
    Ok(())
}

#[test]
fn adoption_requires_the_retained_identity_and_sandbox() -> TestResult {
    let identity = identity(None)?;
    let receipt = receipt(&identity)?;
    assert!(E2bWorkerProvider::verify_adoption(&identity, &receipt, (&identity, Some("sbx1"))).is_ok());
    let mut other = identity.clone();
    other.lease_id = LeaseId::new("lease-2");
    for (retained, sandbox) in [(&other, Some("sbx1")), (&identity, Some("sbx2")), (&identity, None)] {
        assert!(matches!(
            E2bWorkerProvider::verify_adoption(&identity, &receipt, (retained, sandbox)),
            Err(E2bDispatchError::AdoptionMismatch)
        ));
    }
    Ok(())
}

fn active(provider: &E2bWorkerProvider, lease: &LeaseId) -> Result<ActiveLease, Box<dyn std::error::Error>> {
    Ok(ActiveLease {
        access: LeaseAccess::Ready,
        tenant: TenantId::new("t")?,
        gate: provider.lease_gate(lease),
        pause: PauseOwnership::Unowned,
        sandbox_id: "sbx1".to_owned(),
        target: EnvdTarget::for_sandbox(provider.api.config(), "sbx1", None, None)?,
        traffic_token: None,
        traffic_privacy: TrafficPrivacy::default(),
        domain: "e2b.test".to_owned(),
        exposed_ports: Vec::new(),
        lifetime: LeaseLifetime::Requested(60),
        region: None,
    })
}

#[tokio::test]
async fn recovery_never_replaces_a_lease_already_held() -> TestResult {
    let server = MockServer::start().await;
    let provider = build_provider(&server, false)?;
    let lease = LeaseId::new("lease-1");
    provider.install_recovered(&lease, active(&provider, &lease)?).await?;
    assert!(provider.active.lock().await.contains_key("lease-1"));
    assert!(matches!(
        provider.install_recovered(&lease, active(&provider, &lease)?).await,
        Err(E2bDispatchError::RecoveryWouldReplace)
    ));
    Ok(())
}

#[tokio::test]
async fn an_attempt_that_never_allocated_is_left_for_cleanup_on_restart() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let server = MockServer::start().await;
    let provider = build_provider(&server, false)?;
    let directory = tempfile::tempdir()?;
    let identity = identity(None)?;
    let path = directory.path().join("ledger.sqlite");
    let ledger = Arc::new(AttemptLedger::open(
        path.clone(),
        ProviderKind::E2b,
        Some(identity.target.worker_id.clone()),
    )?);
    let request = ProviderCleanupRequest {
        identity,
        intent: CancelIntent::Release,
    };
    ledger
        .cleanup(&request, |_| async { Ok::<(), std::io::Error>(()) })
        .await;
    // A crash between recording the cleanup request and settling it leaves the attempt requested.
    rusqlite::Connection::open(&path)?.execute("UPDATE attempts SET phase=2, backend_cleaned=0", [])?;
    assert_eq!(ledger.try_states().await?.len(), 1);
    provider.configure_dispatch_ledger(ledger)?;
    provider.initialize_dispatch(nats.jetstream().await).await?;
    assert!(provider.active.lock().await.is_empty());
    Ok(())
}
