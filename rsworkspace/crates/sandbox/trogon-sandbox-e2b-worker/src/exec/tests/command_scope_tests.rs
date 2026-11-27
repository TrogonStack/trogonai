use std::time::SystemTime;

use serde::Serialize;
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, DispatchAttemptId, LaunchKey, LeaseEpoch, LeaseId, ProviderKind, TenantId, WorkerId,
};
use trogon_sandbox_protocol::subjects;
use trogon_sandbox_protocol::{
    DispatchSigner, DispatchVerifier, EnforcementMode, EpochMode, VerbScope, VerifiedVerbScope,
};

use super::scripted_store_tests::ScriptedStore;
use super::{TestResult, fixture, owner, run_request};
use crate::exec::command_phase::CommandPhase;
use crate::exec::e2b_exec::E2bExec;
use wiremock::MockServer;

fn full_scope(lease: &str) -> Result<VerbScope, Box<dyn std::error::Error>> {
    Ok(VerbScope {
        tenant: TenantId::new("tenant-1")?,
        lease_id: LeaseId::new(lease),
        worker_id: Some(WorkerId::new("worker-1")?),
        attempt_id: Some(DispatchAttemptId::new("attempt-1")?),
        epoch: Some(LeaseEpoch::new(1)?),
    })
}

fn request(lease: &str) -> Result<CancelCommandLaunchRequest, Box<dyn std::error::Error>> {
    Ok(CancelCommandLaunchRequest {
        lease_id: LeaseId::new(lease),
        launch_key: LaunchKey::new("scoped")?,
    })
}

fn verifier(epoch: EpochMode) -> Result<(DispatchSigner, DispatchVerifier), Box<dyn std::error::Error>> {
    let signer = DispatchSigner::new("scope-fixture", [7; 32]);
    let verifier = DispatchVerifier::new(EnforcementMode::Require)
        .with_epoch_mode(epoch)
        .with_key(signer.key_id(), &signer.public_key_b64())?;
    Ok((signer, verifier))
}

/// A request signed for `subject` with a complete scope, and the exact payload the signature covers.
pub(super) fn signed<T: Serialize>(
    request: &T,
    lease: &LeaseId,
    subject: &str,
) -> Result<(VerifiedVerbScope, Vec<u8>), Box<dyn std::error::Error>> {
    let scope = full_scope(lease.as_str())?;
    let (signer, verifier) = verifier(EpochMode::default())?;
    let payload = serde_json::to_vec_pretty(request)?;
    let header = signer.sign_verb(subject, &payload, &scope)?;
    let verified = verifier.verify_verb_scope(subject, &header, &payload, false, SystemTime::now())?;
    Ok((verified, payload))
}

#[tokio::test]
async fn a_command_owner_needs_a_complete_matching_assignment() -> TestResult {
    let server = MockServer::start().await;
    let (_, sandbox) = fixture(&server)?;
    let request = request("lease-1")?;
    let lease = request.lease_id.clone();

    let subject = subjects::provider_cancel_command_launch(ProviderKind::E2b, &lease);
    let (scope, payload) = signed(&request, &lease, &subject)?;
    let owner = E2bExec::command_owner(&sandbox, &lease, &subject, &request, &scope, &payload)?;
    assert_eq!(owner.sandbox_id, "sbx1");
    assert!(E2bExec::command_owner(&sandbox, &lease, "other", &request, &scope, &payload).is_err());
    assert!(
        E2bExec::command_owner(
            &sandbox,
            &LeaseId::new("lease-2"),
            "subject",
            &request,
            &scope,
            &payload
        )
        .is_err()
    );

    let (_, mut foreign) = fixture(&server)?;
    foreign.sandbox_id = "not a sandbox".to_owned();
    let (scope, payload) = signed(&request, &lease, &subject)?;
    assert!(E2bExec::command_owner(&foreign, &lease, &subject, &request, &scope, &payload).is_err());
    Ok(())
}

#[tokio::test]
async fn a_fresh_launch_waits_for_a_trustworthy_dispatch_fence() -> TestResult {
    let server = MockServer::start().await;
    let (mut exec, sandbox) = fixture(&server)?;
    let store = ScriptedStore::default();
    exec.command_custody = Box::new(store);
    assert!(exec.check_fresh_worker_launch().is_ok());

    let (_, verifier) = verifier(EpochMode::Require)?;
    exec.worker_fence
        .set(Box::leak(Box::new(verifier)))
        .map_err(|_| "the fence is set once")?;
    assert!(exec.check_fresh_worker_launch().is_err());

    let keyed = trogon_sandbox_contract::RunCommandRequest {
        launch_key: Some(LaunchKey::new("fenced")?),
        ..run_request(&["x"])
    };
    let owner = owner("lease-1", "sbx1")?;
    let result = exec.start_controlled(&sandbox, &keyed, owner.clone()).await;
    assert!(result.is_err());
    let stored = exec
        .command_custody
        .load(&owner, &LaunchKey::new("fenced")?)
        .await?
        .ok_or("the reservation is kept")?;
    assert_eq!(stored.record.phase, CommandPhase::Reserved);
    Ok(())
}
