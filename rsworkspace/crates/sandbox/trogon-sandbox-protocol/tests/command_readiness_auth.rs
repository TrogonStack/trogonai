//! Authentication of the signed, worker-addressed command readiness request.

use std::error::Error;
use std::time::SystemTime;

use serde_json::{Value, json};
use trogon_sandbox_contract::{DispatchAttemptId, LeaseEpoch, LeaseId, TenantId, WorkerId};
use trogon_sandbox_protocol::{
    DispatchAuthError, DispatchSigner, DispatchVerifier, EnforcementMode, EpochMode, OP_LEASE_DISPATCH, SignedSubject,
    VerbScope, classify_subject,
};

const ROUTE: &str = "sandbox.provider.local_docker.worker.w1.command_readiness.l1";
const OTHER_WORKER_ROUTE: &str = "sandbox.provider.local_docker.worker.w2.command_readiness.l1";

fn pair() -> Result<(DispatchSigner, DispatchVerifier), Box<dyn Error>> {
    let signer = DispatchSigner::new("readiness-auth-test", [79; 32]);
    let verifier = DispatchVerifier::new(EnforcementMode::Require)
        .with_key(signer.key_id(), &signer.public_key_b64())?
        .with_epoch_mode(EpochMode::Require);
    Ok((signer, verifier))
}

fn scope() -> Result<VerbScope, Box<dyn Error>> {
    Ok(VerbScope {
        tenant: TenantId::new("readiness-tenant")?,
        lease_id: LeaseId::new("l1"),
        worker_id: Some(WorkerId::new("w1")?),
        attempt_id: Some(DispatchAttemptId::new("a1")?),
        epoch: Some(LeaseEpoch::new(7)?),
    })
}

fn body() -> Value {
    json!({
        "tenant": "readiness-tenant", "lease_id": "l1", "provider": "local_docker",
        "target": {"worker_id": "w1", "attempt_id": "a1", "epoch": 7},
        "sandbox_id": "a".repeat(64), "run_as_user": "root",
    })
}

fn admit_dispatch(signer: &DispatchSigner, verifier: &DispatchVerifier) -> Result<(), Box<dyn Error>> {
    let payload = serde_json::to_vec(&json!({
        "lease_id": "l1", "request": {"tenant": "readiness-tenant"},
        "target": {"worker_id": "w1", "attempt_id": "a1", "epoch": 7},
    }))?;
    let signed = signer.sign_payload(&payload, OP_LEASE_DISPATCH)?;
    verifier.decode::<Value>(&signed, OP_LEASE_DISPATCH, Some(&WorkerId::new("w1")?))?;
    Ok(())
}

fn verify(signer: &DispatchSigner, verifier: &DispatchVerifier, scope: &VerbScope) -> Result<(), Box<dyn Error>> {
    let payload = serde_json::to_vec(&body())?;
    let header = signer.sign_verb(ROUTE, &payload, scope)?;
    Ok(verifier.verify_verb(ROUTE, &header, &payload, false, SystemTime::now())?)
}

fn refusal(
    signer: &DispatchSigner,
    verifier: &DispatchVerifier,
    scope: &VerbScope,
) -> Result<DispatchAuthError, Box<dyn Error>> {
    let payload = serde_json::to_vec(&body())?;
    let header = signer.sign_verb(ROUTE, &payload, scope)?;
    verifier
        .verify_verb(ROUTE, &header, &payload, false, SystemTime::now())
        .err()
        .ok_or_else(|| "the request should be refused".into())
}

#[test]
fn readiness_is_a_signed_exact_worker_route_without_a_broadcast_alias() -> Result<(), Box<dyn Error>> {
    assert!(matches!(
        classify_subject(ROUTE),
        Some(SignedSubject::WorkerCommandReadiness { ref provider, ref worker, ref lease })
            if provider == "local_docker" && worker.as_str() == "w1" && lease.as_str() == "l1"
    ));
    assert!(matches!(
        classify_subject("sandbox.provider.local_docker.worker.w1.command_readiness.l1.part"),
        Some(SignedSubject::WorkerCommandReadiness { ref lease, .. }) if lease.as_str() == "l1.part"
    ));
    for invalid in [
        "sandbox.provider.local_docker.command_readiness.l1",
        "sandbox.provider.local_docker.worker.w1.command_readiness",
        "sandbox.provider.local_docker.worker..command_readiness.l1",
        "sandbox.provider..worker.w1.command_readiness.l1",
        "sandbox.provider.local_docker.worker.*.command_readiness.l1",
        "sandbox.provider.local_docker.worker.w1.command_readiness.>",
        "sandbox.provider.local_docker.worker.w1.command_readiness.l1.",
        "sandbox.provider.local docker.worker.w1.command_readiness.l1",
        "sandbox.provider.local_docker.worker.w1.command_readiness.l 1",
    ] {
        assert!(classify_subject(invalid).is_none(), "{invalid}");
    }
    let oversized = format!(
        "sandbox.provider.local_docker.worker.w1.command_readiness.{}",
        "l".repeat(1025)
    );
    assert!(classify_subject(&oversized).is_none());
    let longest = format!(
        "sandbox.provider.local_docker.worker.w1.command_readiness.{}",
        "l".repeat(1024)
    );
    assert!(classify_subject(&longest).is_some());
    Ok(())
}

#[test]
fn readiness_authentication_binds_exact_subject_body_scope_and_nonce() -> Result<(), Box<dyn Error>> {
    let (signer, verifier) = pair()?;
    admit_dispatch(&signer, &verifier)?;
    let payload = serde_json::to_vec(&body())?;
    let expected = scope()?;
    let header = signer.sign_verb(ROUTE, &payload, &expected)?;
    let verified = verifier.verify_verb_scope(ROUTE, &header, &payload, false, SystemTime::now())?;
    assert_eq!(verified.scope(), &expected);
    assert!(verified.matches(ROUTE, &payload));
    assert!(!verified.matches(OTHER_WORKER_ROUTE, &payload));
    let mut changed = body();
    changed["run_as_user"] = "nobody".into();
    assert!(!verified.matches(ROUTE, &serde_json::to_vec(&changed)?));
    let replayed = verifier.verify_verb(ROUTE, &header, &payload, false, SystemTime::now());
    assert!(matches!(replayed, Err(DispatchAuthError::Replayed)));
    Ok(())
}

#[test]
fn readiness_refuses_foreign_subject_lease_and_worker_envelopes() -> Result<(), Box<dyn Error>> {
    let (signer, verifier) = pair()?;
    let payload = serde_json::to_vec(&body())?;
    let header = signer.sign_verb(ROUTE, &payload, &scope()?)?;
    let wrong_route = verifier.verify_verb(OTHER_WORKER_ROUTE, &header, &payload, false, SystemTime::now());
    assert!(matches!(wrong_route, Err(DispatchAuthError::WrongOperation { .. })));
    let foreign_lease = VerbScope {
        lease_id: LeaseId::new("l2"),
        ..scope()?
    };
    assert!(matches!(
        refusal(&signer, &verifier, &foreign_lease)?,
        DispatchAuthError::LeaseMismatch
    ));
    let foreign_worker = VerbScope {
        worker_id: Some(WorkerId::new("w2")?),
        ..scope()?
    };
    assert!(matches!(
        refusal(&signer, &verifier, &foreign_worker)?,
        DispatchAuthError::WrongWorker { ref expected, ref found } if expected == "w1" && found == "w2"
    ));
    let no_worker = VerbScope {
        worker_id: None,
        ..scope()?
    };
    assert!(matches!(
        refusal(&signer, &verifier, &no_worker)?,
        DispatchAuthError::WrongWorker { ref expected, ref found } if expected == "w1" && found.is_empty()
    ));
    Ok(())
}

#[test]
fn readiness_refuses_body_tampering_and_forged_signatures() -> Result<(), Box<dyn Error>> {
    let (signer, verifier) = pair()?;
    let original = serde_json::to_vec(&body())?;
    let header = signer.sign_verb(ROUTE, &original, &scope()?)?;
    let mut changed = body();
    changed["run_as_user"] = "nobody".into();
    let tampered = verifier.verify_verb(ROUTE, &header, &serde_json::to_vec(&changed)?, false, SystemTime::now());
    assert!(matches!(tampered, Err(DispatchAuthError::BodyMismatch)));
    let forged = DispatchSigner::new(signer.key_id(), [83; 32]);
    let header = forged.sign_verb(ROUTE, &original, &scope()?)?;
    let result = verifier.verify_verb(ROUTE, &header, &original, false, SystemTime::now());
    assert!(matches!(result, Err(DispatchAuthError::BadSignature)));
    Ok(())
}

#[test]
fn readiness_refuses_a_transferred_body() -> Result<(), Box<dyn Error>> {
    let (signer, verifier) = pair()?;
    let payload = serde_json::to_vec(&body())?;
    let header = signer.sign_verb(ROUTE, &payload, &scope()?)?;
    let result = verifier.verify_verb(ROUTE, &header, &payload, true, SystemTime::now());
    assert!(matches!(result, Err(DispatchAuthError::Malformed(_))));
    Ok(())
}

#[test]
fn readiness_uses_the_existing_tenant_attempt_and_epoch_fence() -> Result<(), Box<dyn Error>> {
    let (signer, verifier) = pair()?;
    admit_dispatch(&signer, &verifier)?;
    let foreign_tenant = VerbScope {
        tenant: TenantId::new("another-tenant")?,
        ..scope()?
    };
    assert!(matches!(
        refusal(&signer, &verifier, &foreign_tenant)?,
        DispatchAuthError::LeaseTenantConflict
    ));
    let foreign_attempt = VerbScope {
        attempt_id: Some(DispatchAttemptId::new("another-attempt")?),
        ..scope()?
    };
    assert!(matches!(
        refusal(&signer, &verifier, &foreign_attempt)?,
        DispatchAuthError::StaleAttempt
    ));
    let older_epoch = VerbScope {
        epoch: Some(LeaseEpoch::new(6)?),
        ..scope()?
    };
    assert!(matches!(
        refusal(&signer, &verifier, &older_epoch)?,
        DispatchAuthError::StaleEpoch
    ));
    verify(&signer, &verifier, &scope()?)?;
    Ok(())
}

#[test]
fn a_valid_envelope_preserves_foreign_body_fields_for_typed_handler_denial() -> Result<(), Box<dyn Error>> {
    let (signer, verifier) = pair()?;
    admit_dispatch(&signer, &verifier)?;
    for pointer in [
        "/tenant",
        "/lease_id",
        "/provider",
        "/target/worker_id",
        "/target/attempt_id",
        "/target/epoch",
        "/sandbox_id",
    ] {
        let mut foreign = body();
        *foreign.pointer_mut(pointer).ok_or("pointer exists")? = match pointer {
            "/provider" => "e2b".into(),
            "/target/epoch" => 8.into(),
            _ => "foreign-binding".into(),
        };
        let payload = serde_json::to_vec(&foreign)?;
        let header = signer.sign_verb(ROUTE, &payload, &scope()?)?;
        let verified = verifier.verify_verb_scope(ROUTE, &header, &payload, false, SystemTime::now())?;
        assert_eq!(verified.scope(), &scope()?);
        assert!(verified.matches(ROUTE, &payload));
    }
    Ok(())
}

#[test]
fn require_mode_rejects_unsigned_readiness_without_inventing_an_authority() -> Result<(), Box<dyn Error>> {
    let (_, verifier) = pair()?;
    let payload = serde_json::to_vec(&body())?;
    let refused = verifier
        .admit_message_scope(ROUTE, None, &payload)
        .err()
        .ok_or("unsigned readiness is refused")?;
    assert!(matches!(refused, DispatchAuthError::Missing));
    for mode in [EnforcementMode::Warn, EnforcementMode::Off] {
        let admitted = DispatchVerifier::new(mode).admit_message_scope(ROUTE, None, &payload)?;
        assert!(admitted.is_none());
    }
    Ok(())
}

#[test]
fn an_unknown_lease_can_authenticate_without_claiming_held_native_ownership() -> Result<(), Box<dyn Error>> {
    let (signer, verifier) = pair()?;
    verify(&signer, &verifier, &scope()?)
}
