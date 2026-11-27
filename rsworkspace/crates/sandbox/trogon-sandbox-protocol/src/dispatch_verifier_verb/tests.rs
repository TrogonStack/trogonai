use std::time::{Duration, SystemTime};

use trogon_sandbox_contract::{LeaseId, TenantId, WorkerId};

use super::*;
use crate::{EnforcementMode, VerbScope, VerifiedVerbScope};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ROUTE: &str = "sandbox.provider.local_docker.worker.w1.command_readiness.l1";

fn captured(lease: &str, worker: Option<&str>) -> Result<VerifiedVerbScope, Box<dyn std::error::Error>> {
    let now = SystemTime::now();
    Ok(VerifiedVerbScope {
        scope: VerbScope {
            tenant: TenantId::new("tenant")?,
            lease_id: LeaseId::new(lease),
            worker_id: worker.map(WorkerId::new).transpose()?,
            attempt_id: None,
            epoch: None,
        },
        subject: ROUTE.to_owned(),
        body_sha256: String::new(),
        expires_at: unix(now + Duration::from_secs(60)),
    })
}

#[test]
fn a_captured_scope_for_another_lease_than_its_subject_names_is_refused() -> TestResult {
    let verifier = DispatchVerifier::new(EnforcementMode::Off);
    let result = verifier.recheck_command_readiness_scope(&captured("l2", Some("w1"))?, SystemTime::now());
    assert!(matches!(result, Err(DispatchAuthError::LeaseMismatch)), "{result:?}");
    Ok(())
}

#[test]
fn a_captured_scope_for_another_worker_than_its_subject_names_is_refused() -> TestResult {
    let verifier = DispatchVerifier::new(EnforcementMode::Off);
    let other = verifier.recheck_command_readiness_scope(&captured("l1", Some("w9"))?, SystemTime::now());
    assert!(
        matches!(other, Err(DispatchAuthError::WrongWorker { ref expected, ref found }) if expected == "w1" && found == "w9"),
        "{other:?}"
    );
    let none = verifier.recheck_command_readiness_scope(&captured("l1", None)?, SystemTime::now());
    assert!(
        matches!(none, Err(DispatchAuthError::WrongWorker { ref expected, ref found }) if expected == "w1" && found.is_empty()),
        "{none:?}"
    );
    Ok(())
}
