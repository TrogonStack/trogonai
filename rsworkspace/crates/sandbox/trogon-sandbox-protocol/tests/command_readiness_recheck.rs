//! Re-checking a captured command readiness scope against the live lease fence.

use std::error::Error;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use trogon_sandbox_contract::{DispatchAttemptId, LeaseEpoch, LeaseId, TenantId, WorkerId};
use trogon_sandbox_protocol::{
    DispatchAuthError, DispatchSigner, DispatchVerifier, EnforcementMode, EpochMode, OP_LEASE_DISPATCH, VerbScope,
    VerifiedVerbScope,
};

const ROUTE: &str = "sandbox.provider.local_docker.worker.w1.command_readiness.l1";

struct Fixture {
    signer: DispatchSigner,
    verifier: DispatchVerifier,
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let signer = DispatchSigner::new("readiness-recheck", [89; 32]);
        let verifier = DispatchVerifier::new(EnforcementMode::Require)
            .with_key(signer.key_id(), &signer.public_key_b64())?
            .with_epoch_mode(EpochMode::Require)
            .with_fence_file(&directory.path().join("fence.jsonl"))?;
        Ok(Self {
            signer,
            verifier,
            directory,
        })
    }

    fn dispatch(&self, worker: &str, attempt: &str, epoch: u64) -> Result<(), Box<dyn Error>> {
        let payload = serde_json::to_vec(&json!({
            "lease_id": "l1", "request": {"tenant": "readiness-tenant"},
            "target": {"worker_id": worker, "attempt_id": attempt, "epoch": epoch},
        }))?;
        let signed = self.signer.sign_payload(&payload, OP_LEASE_DISPATCH)?;
        self.verifier
            .decode::<Value>(&signed, OP_LEASE_DISPATCH, Some(&WorkerId::new(worker)?))?;
        Ok(())
    }

    fn capture(&self, scope: &VerbScope) -> Result<(VerifiedVerbScope, String, Vec<u8>), Box<dyn Error>> {
        let payload = serde_json::to_vec(&json!({
            "tenant": "readiness-tenant", "lease_id": "l1", "provider": "local_docker",
            "target": {"worker_id": "w1", "attempt_id": "a1", "epoch": 7},
            "sandbox_id": "a".repeat(64), "run_as_user": "root",
        }))?;
        let header = self.signer.sign_verb(ROUTE, &payload, scope)?;
        let captured = self
            .verifier
            .verify_verb_scope(ROUTE, &header, &payload, false, SystemTime::now())?;
        Ok((captured, header, payload))
    }

    fn cleanup(&self) -> Result<(), Box<dyn Error>> {
        let subject = "sandbox.provider.local_docker.worker.w1.cleanup";
        let payload = serde_json::to_vec(&json!({
            "identity": {"lease_id": "l1", "provider": "local_docker",
                "target": {"worker_id": "w1", "attempt_id": "a1", "epoch": 7}},
            "intent": "release",
        }))?;
        let header = self.signer.sign_verb(subject, &payload, &scope(7)?)?;
        self.verifier
            .verify_verb(subject, &header, &payload, false, SystemTime::now())?;
        Ok(())
    }

    fn recheck(&self, captured: &VerifiedVerbScope) -> Result<(), DispatchAuthError> {
        self.verifier
            .recheck_command_readiness_scope(captured, SystemTime::now())
    }
}

fn scope(epoch: u64) -> Result<VerbScope, Box<dyn Error>> {
    Ok(VerbScope {
        tenant: TenantId::new("readiness-tenant")?,
        lease_id: LeaseId::new("l1"),
        worker_id: Some(WorkerId::new("w1")?),
        attempt_id: Some(DispatchAttemptId::new("a1")?),
        epoch: Some(LeaseEpoch::new(epoch)?),
    })
}

#[test]
fn rechecking_captured_readiness_changes_neither_nonce_nor_journal() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.dispatch("w1", "a1", 7)?;
    let (captured, header, payload) = fixture.capture(&scope(7)?)?;
    let path = fixture.directory.path().join("fence.jsonl");
    let before = std::fs::read(&path)?;
    for _ in 0..3 {
        fixture.recheck(&captured)?;
        assert_eq!(std::fs::read(&path)?, before);
    }
    let replayed = fixture
        .verifier
        .verify_verb(ROUTE, &header, &payload, false, SystemTime::now());
    assert!(matches!(replayed, Err(DispatchAuthError::Replayed)));
    Ok(())
}

#[test]
fn captured_readiness_requires_the_exact_current_epoch_including_future_epochs() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.dispatch("w1", "a1", 7)?;
    let (original, _, _) = fixture.capture(&scope(7)?)?;
    let (future, _, _) = fixture.capture(&scope(8)?)?;
    assert!(matches!(fixture.recheck(&future), Err(DispatchAuthError::StaleEpoch)));
    fixture.dispatch("w1", "a1", 8)?;
    assert!(matches!(fixture.recheck(&original), Err(DispatchAuthError::StaleEpoch)));
    let (current, _, _) = fixture.capture(&scope(8)?)?;
    fixture.recheck(&current)?;
    Ok(())
}

#[test]
fn cleaned_forgotten_and_unknown_fences_cannot_retain_readiness_authority() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    let (unknown, _, _) = fixture.capture(&scope(7)?)?;
    assert!(matches!(
        fixture.recheck(&unknown),
        Err(DispatchAuthError::LeaseMismatch)
    ));
    fixture.dispatch("w1", "a1", 7)?;
    let (captured, _, _) = fixture.capture(&scope(7)?)?;
    fixture.cleanup()?;
    assert!(matches!(
        fixture.recheck(&captured),
        Err(DispatchAuthError::StaleAttempt)
    ));
    fixture.verifier.forget_lease(&LeaseId::new("l1"));
    assert!(matches!(
        fixture.recheck(&captured),
        Err(DispatchAuthError::LeaseMismatch)
    ));
    Ok(())
}

#[test]
fn a_new_worker_cannot_lend_its_live_fence_to_the_captured_old_worker() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.dispatch("w1", "a1", 7)?;
    let (captured, _, _) = fixture.capture(&scope(7)?)?;
    fixture.cleanup()?;
    fixture.dispatch("w2", "a2", 8)?;
    assert!(matches!(
        fixture.recheck(&captured),
        Err(DispatchAuthError::WrongWorker { ref expected, ref found }) if expected == "w2" && found == "w1"
    ));
    Ok(())
}

#[test]
fn a_captured_scope_for_another_tenant_cannot_use_the_live_fence() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    let other = VerbScope {
        tenant: TenantId::new("another-tenant")?,
        ..scope(7)?
    };
    let (captured, _, _) = fixture.capture(&other)?;
    fixture.dispatch("w1", "a1", 7)?;
    assert!(matches!(
        fixture.recheck(&captured),
        Err(DispatchAuthError::LeaseTenantConflict)
    ));
    Ok(())
}

#[test]
fn captured_readiness_cannot_outlive_the_signed_envelope() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.dispatch("w1", "a1", 7)?;
    let (captured, _, _) = fixture.capture(&scope(7)?)?;
    fixture.recheck(&captured)?;
    let late = fixture
        .verifier
        .recheck_command_readiness_scope(&captured, SystemTime::now() + Duration::from_secs(600));
    assert!(matches!(late, Err(DispatchAuthError::Expired)));
    Ok(())
}

#[test]
fn incomplete_and_unrelated_verified_scopes_do_not_become_readiness_authority() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.dispatch("w1", "a1", 7)?;
    let without_attempt = VerbScope {
        attempt_id: None,
        ..scope(7)?
    };
    let (captured, _, _) = fixture.capture(&without_attempt)?;
    assert!(matches!(
        fixture.recheck(&captured),
        Err(DispatchAuthError::StaleAttempt)
    ));
    let without_epoch = VerbScope {
        epoch: None,
        ..scope(7)?
    };
    let (captured, _, _) = fixture.capture(&without_epoch)?;
    assert!(matches!(fixture.recheck(&captured), Err(DispatchAuthError::StaleEpoch)));
    let ordinary = "sandbox.provider.local_docker.run_command.l1";
    let payload = br#"{"lease_id":"l1"}"#;
    let header = fixture.signer.sign_verb(ordinary, payload, &scope(7)?)?;
    let captured = fixture
        .verifier
        .verify_verb_scope(ordinary, &header, payload, false, SystemTime::now())?;
    assert!(matches!(
        fixture.recheck(&captured),
        Err(DispatchAuthError::Malformed(_))
    ));
    Ok(())
}

#[test]
fn a_scope_bound_to_another_worker_or_lease_cannot_be_captured() -> Result<(), Box<dyn Error>> {
    let fixture = Fixture::new()?;
    fixture.dispatch("w1", "a1", 7)?;
    let other_worker = VerbScope {
        worker_id: Some(WorkerId::new("w9")?),
        ..scope(7)?
    };
    let refused = fixture.capture(&other_worker).err().ok_or("the capture is refused")?;
    assert!(matches!(
        refused.downcast_ref::<DispatchAuthError>(),
        Some(DispatchAuthError::WrongWorker { .. })
    ));
    let other_lease = VerbScope {
        lease_id: LeaseId::new("l9"),
        ..scope(7)?
    };
    let refused = fixture.capture(&other_lease).err().ok_or("the capture is refused")?;
    assert!(matches!(
        refused.downcast_ref::<DispatchAuthError>(),
        Some(DispatchAuthError::LeaseMismatch)
    ));
    Ok(())
}
