use trogon_sandbox_contract::{
    CancelIntent, DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderKind, TenantId,
    WorkerId,
};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn request(epoch: Option<LeaseEpoch>) -> Result<ProviderCleanupRequest, Box<dyn std::error::Error>> {
    Ok(ProviderCleanupRequest {
        identity: LeaseAttemptIdentity {
            lease_id: LeaseId::new("lease-1"),
            provider: ProviderKind::Vercel,
            target: DispatchTarget {
                worker_id: WorkerId::new("worker-1")?,
                attempt_id: DispatchAttemptId::new("attempt-1")?,
                epoch,
            },
        },
        intent: CancelIntent::Release,
    })
}

fn scope(epoch: Option<LeaseEpoch>) -> Result<VerbScope, Box<dyn std::error::Error>> {
    Ok(VerbScope {
        tenant: TenantId::new("tenant-1")?,
        lease_id: LeaseId::new("lease-1"),
        worker_id: Some(WorkerId::new("worker-1")?),
        attempt_id: Some(DispatchAttemptId::new("attempt-1")?),
        epoch,
    })
}

#[test]
fn a_scope_signed_for_the_assignment_names_it() -> TestResult {
    let epoch = Some(LeaseEpoch::new(3)?);
    assert!(names_assignment(&scope(epoch)?, &request(epoch)?));
    assert!(names_assignment(&scope(None)?, &request(None)?));
    Ok(())
}

#[test]
fn a_scope_signed_for_another_epoch_does_not() -> TestResult {
    assert!(!names_assignment(&scope(Some(LeaseEpoch::new(3)?))?, &request(None)?));
    assert!(!names_assignment(
        &scope(Some(LeaseEpoch::new(3)?))?,
        &request(Some(LeaseEpoch::new(4)?))?
    ));
    Ok(())
}

#[test]
fn a_scope_signed_for_another_lease_worker_or_attempt_does_not() -> TestResult {
    let wanted = request(None)?;
    let mut other_lease = scope(None)?;
    other_lease.lease_id = LeaseId::new("lease-2");
    let mut other_worker = scope(None)?;
    other_worker.worker_id = Some(WorkerId::new("worker-2")?);
    let mut other_attempt = scope(None)?;
    other_attempt.attempt_id = Some(DispatchAttemptId::new("attempt-2")?);
    let mut unbound = scope(None)?;
    unbound.worker_id = None;
    for scope in [other_lease, other_worker, other_attempt, unbound] {
        assert!(!names_assignment(&scope, &wanted));
    }
    Ok(())
}
