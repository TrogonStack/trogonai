use std::sync::Arc;
use std::time::SystemTime;

use async_nats::jetstream::Context;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelIntent, CancelOwnedLeaseRequest, DispatchAttemptId, DispatchTarget, LaunchKey,
    LeaseAttemptIdentity, LeaseEpoch, LeaseError, LeaseId, ProviderCleanupRequest, ProviderCleanupResponse,
    ProviderKind, ProviderLease, SandboxId, TenantId, WorkerId,
};
use trogon_sandbox_protocol::{
    AttemptLedger, DispatchSigner, DispatchVerifier, EnforcementMode, Provider, VerbScope, VerifiedVerbScope, subjects,
};
use wiremock::{MockGuard, MockServer, ResponseTemplate};

use super::tests::{TestResult, build_provider, created, mount, request};
use super::{E2bDispatchError, E2bWorkerProvider};

const WORKER: &str = "scope-worker";

struct Rig {
    _nats: JetStreamTestServer,
    _directory: tempfile::TempDir,
    context: Context,
    server: MockServer,
    provider: E2bWorkerProvider,
    ledger: Arc<AttemptLedger>,
}

async fn rig() -> Result<Rig, Box<dyn std::error::Error>> {
    let nats = JetStreamTestServer::start().await;
    let context = nats.jetstream().await;
    let server = MockServer::start().await;
    let provider = build_provider(&server, true)?;
    provider.configure_snapshots(context.clone())?;
    let directory = tempfile::tempdir()?;
    let ledger = Arc::new(AttemptLedger::open(
        directory.path().join("ledger.sqlite"),
        ProviderKind::E2b,
        Some(WorkerId::new(WORKER)?),
    )?);
    provider.configure_dispatch_ledger(ledger.clone())?;
    provider.assignments.configure(context.clone()).await?;
    Ok(Rig {
        _nats: nats,
        _directory: directory,
        context,
        server,
        provider,
        ledger,
    })
}

fn identity(lease: &str, epoch: Option<LeaseEpoch>) -> Result<LeaseAttemptIdentity, Box<dyn std::error::Error>> {
    Ok(LeaseAttemptIdentity {
        lease_id: LeaseId::new(lease),
        provider: ProviderKind::E2b,
        target: DispatchTarget {
            worker_id: WorkerId::new(WORKER)?,
            attempt_id: DispatchAttemptId::new("attempt-1")?,
            epoch,
        },
    })
}

fn epoch() -> Result<Option<LeaseEpoch>, Box<dyn std::error::Error>> {
    Ok(Some(LeaseEpoch::new(1)?))
}

fn signed(scope: &VerbScope) -> Result<VerifiedVerbScope, Box<dyn std::error::Error>> {
    let signer = DispatchSigner::new("targeted-fixture", [9; 32]);
    let verifier =
        DispatchVerifier::new(EnforcementMode::Require).with_key(signer.key_id(), &signer.public_key_b64())?;
    let payload = serde_json::to_vec_pretty(&CancelCommandLaunchRequest {
        lease_id: scope.lease_id.clone(),
        launch_key: LaunchKey::new("scoped")?,
    })?;
    let subject = subjects::provider_cancel_command_launch(ProviderKind::E2b, &scope.lease_id);
    let header = signer.sign_verb(&subject, &payload, scope)?;
    Ok(verifier.verify_verb_scope(&subject, &header, &payload, false, SystemTime::now())?)
}

fn scope(
    tenant: &str,
    lease: &str,
    attempt: Option<&str>,
    epoch: Option<LeaseEpoch>,
) -> Result<VerifiedVerbScope, Box<dyn std::error::Error>> {
    signed(&VerbScope {
        tenant: TenantId::new(tenant)?,
        lease_id: LeaseId::new(lease),
        worker_id: Some(WorkerId::new(WORKER)?),
        attempt_id: attempt.map(DispatchAttemptId::new).transpose()?,
        epoch,
    })
}

fn cleanup_request(identity: LeaseAttemptIdentity) -> ProviderCleanupRequest {
    ProviderCleanupRequest {
        identity,
        intent: CancelIntent::Release,
    }
}

fn denied(result: Result<Option<crate::LeaseSandbox>, LeaseError>) -> bool {
    matches!(result, Err(LeaseError::InvalidSpec { .. }))
}

async fn dispatched(rig: &Rig, lease: &str) -> Result<LeaseAttemptIdentity, Box<dyn std::error::Error>> {
    let _create: MockGuard = created(&rig.server).await;
    let identity = identity(lease, epoch()?)?;
    let receipt = rig
        .provider
        .dispatch_targeted(&request()?, &identity, &rig.context)
        .await?;
    assert_eq!(receipt.sandbox_id.as_str(), "sbx1");
    Ok(identity)
}

#[tokio::test]
async fn a_dispatch_without_an_epoch_is_refused() -> TestResult {
    let rig = rig().await?;
    let result = rig
        .provider
        .dispatch_targeted(&request()?, &identity("lease-1", None)?, &rig.context)
        .await;
    assert!(matches!(result, Err(E2bDispatchError::TargetMismatch)));
    Ok(())
}

#[tokio::test]
async fn a_dispatched_lease_serves_only_commands_signed_for_its_assignment() -> TestResult {
    let rig = rig().await?;
    let identity = dispatched(&rig, "lease-1").await?;
    let lease = identity.lease_id.clone();
    let good = scope("t", "lease-1", Some("attempt-1"), epoch()?)?;
    assert!(
        rig.provider
            .sandbox_for_command(&lease, Some(&good), true)
            .await?
            .is_some()
    );
    assert!(rig.provider.sandbox_for_command(&lease, None, false).await?.is_some());
    assert!(denied(rig.provider.sandbox_for_command(&lease, None, true).await));
    assert!(
        rig.provider
            .sandbox_for_command(&LeaseId::new("other"), Some(&good), true)
            .await?
            .is_none()
    );
    let foreign_tenant = scope("other", "lease-1", Some("attempt-1"), epoch()?)?;
    assert!(denied(
        rig.provider
            .sandbox_for_command(&lease, Some(&foreign_tenant), true)
            .await
    ));
    let no_epoch = scope("t", "lease-1", Some("attempt-1"), None)?;
    assert!(denied(
        rig.provider.sandbox_for_command(&lease, Some(&no_epoch), true).await
    ));
    let no_attempt = scope("t", "lease-1", None, epoch()?)?;
    assert!(denied(
        rig.provider.sandbox_for_command(&lease, Some(&no_attempt), true).await
    ));
    let unknown_attempt = scope("t", "lease-1", Some("attempt-2"), epoch()?)?;
    assert!(denied(
        rig.provider
            .sandbox_for_command(&lease, Some(&unknown_attempt), true)
            .await
    ));
    Ok(())
}

#[tokio::test]
async fn a_lease_whose_attempt_is_being_cleaned_stops_serving_commands() -> TestResult {
    let rig = rig().await?;
    let identity = dispatched(&rig, "lease-1").await?;
    rig.ledger
        .cleanup(&cleanup_request(identity.clone()), |_| async {
            Err::<(), std::io::Error>(std::io::Error::other("vendor down"))
        })
        .await;
    let good = scope("t", "lease-1", Some("attempt-1"), epoch()?)?;
    assert!(denied(
        rig.provider
            .sandbox_for_command(&identity.lease_id, Some(&good), true)
            .await
    ));
    Ok(())
}

#[tokio::test]
async fn a_lease_held_without_a_dispatch_ledger_serves_unfenced_commands_only() -> TestResult {
    let server = MockServer::start().await;
    let _create = created(&server).await;
    let provider = build_provider(&server, true)?;
    let lease = LeaseId::new("l1");
    provider.lease(&request()?, &lease).await?;
    let unfenced = signed(&VerbScope {
        tenant: TenantId::new("t")?,
        lease_id: lease.clone(),
        worker_id: None,
        attempt_id: None,
        epoch: None,
    })?;
    assert!(denied(
        provider.sandbox_for_command(&lease, Some(&unfenced), true).await
    ));
    assert!(
        provider
            .sandbox_for_command(&lease, Some(&unfenced), false)
            .await?
            .is_some()
    );
    Ok(())
}

#[tokio::test]
async fn a_lease_the_protocol_cannot_name_is_dispatched_without_lifecycle_names() -> TestResult {
    let rig = rig().await?;
    let _create = created(&rig.server).await;
    let unnameable = identity("bad\u{1}lease", epoch()?)?;
    let receipt = rig
        .provider
        .dispatch_targeted(&request()?, &unnameable, &rig.context)
        .await?;
    assert_eq!(receipt.sandbox_id.as_str(), "sbx1");
    Ok(())
}

#[tokio::test]
async fn a_failed_allocation_for_an_unnameable_lease_is_refused() -> TestResult {
    let rig = rig().await?;
    let _create = mount(&rig.server, "POST", "/v2/sandboxes", ResponseTemplate::new(500)).await;
    let unnameable = identity("bad\u{2}lease", epoch()?)?;
    let result = rig
        .provider
        .dispatch_targeted(&request()?, &unnameable, &rig.context)
        .await;
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn a_cleanup_needs_the_ledger_and_the_named_worker() -> TestResult {
    let server = MockServer::start().await;
    let bare = build_provider(&server, true)?;
    let tenant = TenantId::new("t")?;
    let response = bare
        .cleanup_targeted(&cleanup_request(identity("lease-1", epoch()?)?), &tenant)
        .await;
    assert!(matches!(response, ProviderCleanupResponse::Failed { .. }));

    let rig = rig().await?;
    let mut foreign = identity("lease-1", epoch()?)?;
    foreign.target.worker_id = WorkerId::new("another-worker")?;
    let response = rig.provider.cleanup_targeted(&cleanup_request(foreign), &tenant).await;
    assert!(matches!(response, ProviderCleanupResponse::Failed { .. }));

    let reserved = identity("lease-9", epoch()?)?;
    rig.provider.assignments.reserve(&tenant, &reserved, None).await?;
    let other_tenant = TenantId::new("other")?;
    let response = rig
        .provider
        .cleanup_targeted(&cleanup_request(reserved), &other_tenant)
        .await;
    assert!(matches!(response, ProviderCleanupResponse::Failed { .. }));
    Ok(())
}

#[tokio::test]
async fn a_cleanup_deletes_the_sandbox_the_dispatch_created() -> TestResult {
    let rig = rig().await?;
    let identity = dispatched(&rig, "lease-1").await?;
    let tenant = TenantId::new("t")?;
    let assignment = rig
        .provider
        .assignments
        .load(&identity)
        .await?
        .ok_or("assignment missing")?;
    assert!(E2bWorkerProvider::names_held(&assignment, &tenant, "sbx1"));
    assert!(!E2bWorkerProvider::names_held(
        &assignment,
        &TenantId::new("other")?,
        "sbx1"
    ));
    assert!(!E2bWorkerProvider::names_held(&assignment, &tenant, "sbx2"));
    let _delete = mount(&rig.server, "DELETE", "/sandboxes/sbx1", ResponseTemplate::new(204)).await;
    let _state = mount(&rig.server, "GET", "/sandboxes/sbx1", ResponseTemplate::new(404)).await;
    let response = rig
        .provider
        .cleanup_targeted(&cleanup_request(identity.clone()), &tenant)
        .await;
    assert!(matches!(response, ProviderCleanupResponse::Cleaned { .. }));
    assert!(rig.provider.sandbox_for_lease(&identity.lease_id).await.is_none());
    Ok(())
}

#[tokio::test]
async fn a_cleanup_never_touches_a_lease_held_for_another_tenant() -> TestResult {
    let rig = rig().await?;
    let identity = dispatched(&rig, "lease-1").await?;
    if let Some(held) = rig.provider.active.lock().await.get_mut("lease-1") {
        held.tenant = TenantId::new("other")?;
    }
    let response = rig
        .provider
        .cleanup_targeted(&cleanup_request(identity), &TenantId::new("t")?)
        .await;
    assert!(matches!(response, ProviderCleanupResponse::Failed { .. }));
    Ok(())
}

#[tokio::test]
async fn a_cleanup_of_an_attempt_without_a_receipt_fails() -> TestResult {
    let rig = rig().await?;
    let identity = identity("lease-1", epoch()?)?;
    let attempt = rig
        .ledger
        .dispatch(&identity, || async {
            Err::<ProviderLease, std::io::Error>(std::io::Error::other("no"))
        })
        .await;
    assert!(attempt.is_err());
    let response = rig
        .provider
        .cleanup_targeted(&cleanup_request(identity), &TenantId::new("t")?)
        .await;
    assert!(matches!(response, ProviderCleanupResponse::Failed { .. }));
    Ok(())
}

#[tokio::test]
async fn a_cleanup_of_a_lease_this_process_does_not_hold_skips_a_sandbox_already_gone() -> TestResult {
    let rig = rig().await?;
    let identity = identity("lease-1", epoch()?)?;
    let adopted = ProviderLease {
        sandbox_id: SandboxId::new("sbx1")?,
        evidence_subject: "evidence".to_owned(),
        worker_id: None,
        attempt_id: None,
        epoch: None,
    };
    let recovery = crate::assignments::Recovery::from_request(&request()?);
    rig.provider
        .assignments
        .reserve(&TenantId::new("t")?, &identity, Some(recovery))
        .await?;
    rig.ledger
        .dispatch(&identity, || async { Ok::<_, std::io::Error>(adopted) })
        .await?;
    let _delete = mount(&rig.server, "DELETE", "/sandboxes/sbx1", ResponseTemplate::new(204)).await;
    let _state = mount(&rig.server, "GET", "/sandboxes/sbx1", ResponseTemplate::new(404)).await;
    let owned = CancelOwnedLeaseRequest {
        tenant: TenantId::new("t")?,
        lease_id: identity.lease_id.clone(),
        sandbox_id: SandboxId::new("sbx1")?,
    };
    rig.provider.cleanup_receipts.begin(&owned).await?;
    assert!(rig.provider.cancel_owned(&owned).await.is_some());
    let response = rig
        .provider
        .cleanup_targeted(&cleanup_request(identity), &TenantId::new("t")?)
        .await;
    assert!(
        matches!(response, ProviderCleanupResponse::Cleaned { .. }),
        "{response:?}"
    );
    Ok(())
}
