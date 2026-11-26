//! Signed per-lease requests: verification, replay, the tenant and attempt fence, cleanup, and subject classification.

use std::fmt::Write as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_nats::HeaderMap;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signer as _, SigningKey};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use trogon_sandbox_contract::{DispatchAttemptId, DispatchAuth, FileSha256, LeaseId, TenantId, WorkerId};
use trogon_sandbox_protocol::{
    AUTH_HEADER, DispatchAuthError, DispatchMalformedReason, DispatchSigner, DispatchVerifier, EnforcementMode,
    OP_LEASE_DISPATCH, SignedSubject, VerbScope, classify_subject,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const RUN: &str = "sandbox.provider.firecracker.run_command.l1";
const CLEANUP: &str = "sandbox.provider.firecracker.worker.w1.cleanup";
const FORGE_SEED: [u8; 32] = [11u8; 32];

fn random_seed() -> [u8; 32] {
    let mut seed = [0u8; 32];
    seed[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    seed[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    seed
}

fn pair(mode: EnforcementMode) -> Result<(DispatchSigner, DispatchVerifier), Box<dyn std::error::Error>> {
    let signer = DispatchSigner::new("k1", random_seed());
    let verifier = DispatchVerifier::new(mode).with_key("k1", &signer.public_key_b64())?;
    Ok((signer, verifier))
}

fn scope(tenant: &str, lease: &str, attempt: Option<&str>) -> Result<VerbScope, Box<dyn std::error::Error>> {
    Ok(VerbScope {
        epoch: None,
        tenant: TenantId::new(tenant)?,
        lease_id: LeaseId::new(lease),
        worker_id: Some(WorkerId::new("w1")?),
        attempt_id: attempt.map(DispatchAttemptId::new).transpose()?,
    })
}

fn cleanup_body(lease: &str, attempt: &str) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&json!({
        "identity": {"lease_id": lease, "provider": "firecracker",
            "target": {"worker_id": "w1", "attempt_id": attempt}},
        "intent": "release",
    }))
}

fn dispatch_for(signer: &DispatchSigner, tenant: &str, attempt: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let payload = serde_json::to_vec(&json!({
        "lease_id": "l1", "request": {"tenant": tenant},
        "target": {"worker_id": "w1", "attempt_id": attempt},
    }))?;
    Ok(signer.sign_payload(&payload, OP_LEASE_DISPATCH)?)
}

fn check(verifier: &DispatchVerifier, payload: &[u8]) -> Result<(), DispatchAuthError> {
    let mut value: Value = serde_json::from_slice(payload).map_err(DispatchMalformedReason::Json)?;
    verifier.verify_value(&mut value, OP_LEASE_DISPATCH, None, SystemTime::now())
}

fn verify(verifier: &DispatchVerifier, subject: &str, header: &str, body: &[u8]) -> Result<(), DispatchAuthError> {
    verifier.verify_verb(subject, header, body, false, SystemTime::now())
}

fn headers_for(header: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(AUTH_HEADER, header);
    headers
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn signing_input(auth: &DispatchAuth) -> Vec<u8> {
    let mut out = String::from("sandbox-dispatch-auth-v1\n");
    let mut field = |name: &str, value: &str| {
        let _ = writeln!(out, "{name}={}:{value}", value.len());
    };
    field("version", &auth.version.to_string());
    field("key_id", &auth.key_id);
    field("operation", &auth.operation);
    field("tenant", auth.tenant.as_str());
    field("lease_id", auth.lease_id.as_str());
    field("worker_id", auth.worker_id.as_ref().map_or("", WorkerId::as_str));
    field(
        "attempt_id",
        auth.attempt_id.as_ref().map_or("", |attempt| attempt.as_str()),
    );
    if let Some(nonce) = &auth.nonce {
        field("nonce", nonce);
    }
    if let Some(epoch) = auth.epoch {
        field("epoch", &epoch.get().to_string());
    }
    field("expires_at", &auth.expires_at.to_string());
    field("body_sha256", auth.body_sha256.as_str());
    out.into_bytes()
}

fn forge_verifier(mode: EnforcementMode) -> Result<DispatchVerifier, Box<dyn std::error::Error>> {
    let signer = DispatchSigner::new("k1", FORGE_SEED);
    Ok(DispatchVerifier::new(mode).with_key("k1", &signer.public_key_b64())?)
}

/// A header for `RUN` carrying `body`, signed by hand so each field can be chosen.
fn forged_header(body: &[u8], mutate: impl FnOnce(&mut DispatchAuth)) -> Result<String, Box<dyn std::error::Error>> {
    let mut auth = DispatchAuth {
        version: 1,
        key_id: "k1".to_owned(),
        operation: RUN.to_owned(),
        tenant: TenantId::new("t1")?,
        lease_id: LeaseId::new("l1"),
        worker_id: None,
        attempt_id: None,
        nonce: Some(uuid::Uuid::new_v4().simple().to_string()),
        epoch: None,
        expires_at: 1_000_000,
        body_sha256: FileSha256::new(hex(&Sha256::digest(body)))?,
        signature: String::new(),
    };
    mutate(&mut auth);
    auth.signature = B64.encode(
        SigningKey::from_bytes(&FORGE_SEED)
            .sign(&signing_input(&auth))
            .to_bytes(),
    );
    Ok(serde_json::to_string(&auth)?)
}

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

#[test]
fn an_authenticated_verb_scope_is_bound_to_its_exact_subject_and_body() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let body = br#"{"lease_id":"l1","launch_key":"owned"}"#;
    let expected = scope("tenant", "l1", Some("attempt"))?;
    let header = signer.sign_verb(RUN, body, &expected)?;
    let verified = verifier.verify_verb_scope(RUN, &header, body, false, SystemTime::now())?;
    assert_eq!(verified.scope(), &expected);
    assert!(verified.matches(RUN, body));
    assert!(!verified.matches(CLEANUP, body));
    assert!(!verified.matches(RUN, b"changed"));
    let result = verifier.verify_verb_scope(RUN, &header, body, false, SystemTime::now());
    assert!(matches!(result, Err(DispatchAuthError::Replayed)), "{result:?}");
    Ok(())
}

#[test]
fn warn_and_off_admission_never_manufacture_authenticated_authority() -> TestResult {
    let body = br#"{"lease_id":"l1"}"#;
    for mode in [EnforcementMode::Off, EnforcementMode::Warn] {
        let verifier = DispatchVerifier::new(mode);
        assert!(verifier.admit_message_scope(RUN, None, body)?.is_none());
        let (signer, verifier) = pair(mode)?;
        let mut header: Value =
            serde_json::from_str(&signer.sign_verb(RUN, body, &scope("tenant", "l1", Some("attempt"))?)?)?;
        header["signature"] = "invalid".into();
        let headers = headers_for(&header.to_string());
        assert!(verifier.admit_message_scope(RUN, Some(&headers), body)?.is_none());
    }
    Ok(())
}

#[test]
fn require_admission_returns_the_verified_scope() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let body = br#"{"lease_id":"l1"}"#;
    let expected = scope("tenant", "l1", Some("attempt"))?;
    let header = signer.sign_verb(RUN, body, &expected)?;
    let verified = verifier
        .admit_message_scope(RUN, Some(&headers_for(&header)), body)?
        .ok_or("a verified request must return its scope")?;
    assert_eq!(verified.scope(), &expected);
    assert!(verified.matches(RUN, body));
    let clone = verified.clone();
    assert_eq!(clone.scope(), verified.scope());
    assert!(format!("{verified:?}").contains("VerifiedVerbScope"));
    Ok(())
}

#[test]
fn subjects_are_classified() -> TestResult {
    assert_eq!(
        classify_subject(RUN),
        Some(SignedSubject::LeaseVerb {
            lease: LeaseId::new("l1")
        })
    );
    assert_eq!(classify_subject(CLEANUP), Some(SignedSubject::Cleanup));
    assert_eq!(
        classify_subject("sandbox.provider.substrate.purge_named"),
        Some(SignedSubject::PurgeNamed)
    );
    for other in [
        "sandbox.provider.firecracker.lease",
        "sandbox.provider.firecracker.worker.w1.lease",
        "sandbox.provider.firecracker.heartbeat.l1",
        "sandbox.provider.firecracker.run_command",
        "sandbox.lease.request",
    ] {
        assert_eq!(classify_subject(other), None, "{other}");
    }
    Ok(())
}

#[test]
fn lease_verbs_keep_a_dotted_lease_and_refuse_empty_tokens() {
    assert_eq!(
        classify_subject("sandbox.provider.mock.pause.tenant.7.l1"),
        Some(SignedSubject::LeaseVerb {
            lease: LeaseId::new("tenant.7.l1")
        })
    );
    for refused in [
        "sandbox.provider.mock.pause.",
        "sandbox.provider.mock.pause.a..b",
        "sandbox.provider.mock.pause",
        "sandbox.provider.mock.unknown_verb.l1",
        "other.provider.mock.pause.l1",
        "sandbox.other.mock.pause.l1",
        "",
    ] {
        assert_eq!(classify_subject(refused), None, "{refused}");
    }
}

#[test]
fn ssh_export_and_snapshot_deletion_subjects_are_classified() {
    let session = Some(SignedSubject::SshSession {
        lease: LeaseId::new("l1"),
    });
    assert_eq!(classify_subject("sandbox.ssh.attach.l1"), session);
    assert_eq!(classify_subject("sandbox.ssh.detach.l1"), session);
    assert_eq!(
        classify_subject("sandbox.ssh.attach.a.b"),
        Some(SignedSubject::SshSession {
            lease: LeaseId::new("a.b")
        })
    );
    assert_eq!(
        classify_subject("sandbox.worker.w1.snapshot.export"),
        Some(SignedSubject::SnapshotExport)
    );
    assert_eq!(
        classify_subject("sandbox.provider.e2b.native_snapshot_delete"),
        Some(SignedSubject::NativeSnapshotDelete {
            provider: "e2b".to_owned()
        })
    );
    for other in [
        "sandbox.ssh.attach",
        "sandbox.ssh.attach.",
        "sandbox.ssh.attach.a..b",
        "sandbox.ssh.stream.s1.to_worker",
        "sandbox.worker.w1.snapshot.get",
        "sandbox.worker..snapshot.export",
        "sandbox.provider..native_snapshot_delete",
    ] {
        assert_eq!(classify_subject(other), None, "{other}");
    }
}

#[test]
fn command_readiness_subjects_are_classified_with_a_checked_provider_worker_and_lease() -> TestResult {
    let expected = Some(SignedSubject::WorkerCommandReadiness {
        provider: "fire-cracker_2".to_owned(),
        worker: WorkerId::new("w1")?,
        lease: LeaseId::new("tenant.l1"),
    });
    assert_eq!(
        classify_subject("sandbox.provider.fire-cracker_2.worker.w1.command_readiness.tenant.l1"),
        expected
    );
    let long_lease = "x".repeat(1024);
    assert!(
        classify_subject(&format!(
            "sandbox.provider.mock.worker.w1.command_readiness.{long_lease}"
        ))
        .is_some()
    );
    let too_long = "x".repeat(1025);
    for refused in [
        "sandbox.provider.mock.worker.w1.command_readiness".to_owned(),
        "sandbox.provider.mock.worker.w1.command_readiness.".to_owned(),
        "sandbox.provider.mock.worker.w1.command_readiness.a..b".to_owned(),
        "sandbox.provider.mock.worker.w1.command_readiness.a*".to_owned(),
        "sandbox.provider.mock.worker.w1.command_readiness.a>".to_owned(),
        "sandbox.provider.mock.worker.w1.command_readiness.a b".to_owned(),
        "sandbox.provider.mock.worker.w1.command_readiness.a\tb".to_owned(),
        "sandbox.provider.mock.worker.bad worker.command_readiness.l1".to_owned(),
        "sandbox.provider.mock.worker..command_readiness.l1".to_owned(),
        "sandbox.provider..worker.w1.command_readiness.l1".to_owned(),
        "sandbox.provider.bad!.worker.w1.command_readiness.l1".to_owned(),
        format!("sandbox.provider.mock.worker.w1.command_readiness.{too_long}"),
    ] {
        assert_eq!(classify_subject(&refused), None, "{refused}");
    }
    Ok(())
}

#[test]
fn a_signed_verb_verifies_once() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let body = br#"{"lease_id":"l1","command":["ls"]}"#;
    let header = signer.sign_verb(RUN, body, &scope("t1", "l1", Some("a1"))?)?;
    verify(&verifier, RUN, &header, body)?;
    let result = verify(&verifier, RUN, &header, body);
    assert!(matches!(result, Err(DispatchAuthError::Replayed)), "{result:?}");
    Ok(())
}

#[test]
fn every_signed_verb_carries_a_fresh_nonce() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let body = b"{}";
    let first = signer.sign_verb(RUN, body, &scope("t1", "l1", None)?)?;
    let second = signer.sign_verb(RUN, body, &scope("t1", "l1", None)?)?;
    let nonce = |header: &str| -> Result<String, Box<dyn std::error::Error>> {
        let value: Value = serde_json::from_str(header)?;
        Ok(value["nonce"].as_str().ok_or("a verb carries a nonce")?.to_owned())
    };
    assert_ne!(nonce(&first)?, nonce(&second)?);
    verify(&verifier, RUN, &first, body)?;
    verify(&verifier, RUN, &second, body)?;
    Ok(())
}

#[test]
fn a_verb_without_a_nonce_is_refused() -> TestResult {
    let verifier = forge_verifier(EnforcementMode::Require)?;
    let header = forged_header(b"{}", |auth| auth.nonce = None)?;
    let result = verifier.verify_verb(RUN, &header, b"{}", false, at(1_000));
    assert!(
        matches!(
            result,
            Err(DispatchAuthError::Malformed(DispatchMalformedReason::Nonce))
        ),
        "{result:?}"
    );
    Ok(())
}

#[test]
fn a_hand_signed_verb_verifies_and_replays_are_refused() -> TestResult {
    let verifier = forge_verifier(EnforcementMode::Require)?;
    let header = forged_header(b"{}", |auth| auth.nonce = Some("fixed-nonce".to_owned()))?;
    verifier.verify_verb(RUN, &header, b"{}", false, at(1_000))?;
    let again = forged_header(b"{}", |auth| auth.nonce = Some("fixed-nonce".to_owned()))?;
    let result = verifier.verify_verb(RUN, &again, b"{}", false, at(1_000));
    assert!(matches!(result, Err(DispatchAuthError::Replayed)), "{result:?}");
    let fresh = forged_header(b"{}", |auth| auth.nonce = Some("another-nonce".to_owned()))?;
    verifier.verify_verb(RUN, &fresh, b"{}", false, at(1_000))?;
    Ok(())
}

#[test]
fn an_unsupported_version_malformed_signature_or_header_is_refused() -> TestResult {
    let verifier = forge_verifier(EnforcementMode::Require)?;
    let header = forged_header(b"{}", |auth| auth.version = 2)?;
    let result = verifier.verify_verb(RUN, &header, b"{}", false, at(1_000));
    assert!(
        matches!(
            result,
            Err(DispatchAuthError::Malformed(
                DispatchMalformedReason::UnsupportedVersion { version: 2 }
            ))
        ),
        "{result:?}"
    );
    let mut value: Value = serde_json::from_str(&forged_header(b"{}", |_| {})?)?;
    value["signature"] = "@@".into();
    let result = verifier.verify_verb(RUN, &value.to_string(), b"{}", false, at(1_000));
    assert!(
        matches!(
            result,
            Err(DispatchAuthError::Malformed(DispatchMalformedReason::Signature))
        ),
        "{result:?}"
    );
    for garbage in ["not json", "{}", "[]", ""] {
        let result = verifier.verify_verb(RUN, garbage, b"{}", false, at(1_000));
        assert!(
            matches!(
                result,
                Err(DispatchAuthError::Malformed(DispatchMalformedReason::Json(_)))
            ),
            "{garbage}: {result:?}"
        );
    }
    Ok(())
}

#[test]
fn a_header_for_a_subject_that_is_not_a_signed_request_is_refused() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let subject = "sandbox.lease.request";
    let header = signer.sign_verb(subject, b"{}", &scope("t1", "l1", None)?)?;
    let result = verify(&verifier, subject, &header, b"{}");
    assert!(matches!(result, Err(DispatchAuthError::Missing)), "{result:?}");
    Ok(())
}

#[test]
fn a_verb_bound_to_another_subject_body_or_key_is_refused() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let body = b"{}";
    let header = signer.sign_verb(RUN, body, &scope("t1", "l1", None)?)?;
    let other = "sandbox.provider.firecracker.run_command.l2";
    let result = verify(&verifier, other, &header, body);
    assert!(
        matches!(&result, Err(DispatchAuthError::WrongOperation { expected, found })
            if expected == other && found == RUN),
        "{result:?}"
    );
    let result = verify(&verifier, RUN, &header, b"{\"x\":1}");
    assert!(matches!(result, Err(DispatchAuthError::BodyMismatch)), "{result:?}");
    let forged = DispatchSigner::new("k1", random_seed()).sign_verb(RUN, body, &scope("t1", "l1", None)?)?;
    let result = verify(&verifier, RUN, &forged, body);
    assert!(matches!(result, Err(DispatchAuthError::BadSignature)), "{result:?}");
    let unknown = DispatchSigner::new("k9", random_seed()).sign_verb(RUN, body, &scope("t1", "l1", None)?)?;
    let result = verify(&verifier, RUN, &unknown, body);
    assert!(
        matches!(&result, Err(DispatchAuthError::UnknownKey(id)) if id == "k9"),
        "{result:?}"
    );
    let mut tampered: DispatchAuth = serde_json::from_str(&header)?;
    tampered.tenant = TenantId::new("t2")?;
    let result = verify(&verifier, RUN, &serde_json::to_string(&tampered)?, body);
    assert!(matches!(result, Err(DispatchAuthError::BadSignature)), "{result:?}");
    // A signature for lease l2 cannot ride on lease l1's subject.
    let mismatched = signer.sign_verb(RUN, body, &scope("t1", "l2", None)?)?;
    let result = verify(&verifier, RUN, &mismatched, body);
    assert!(matches!(result, Err(DispatchAuthError::LeaseMismatch)), "{result:?}");
    Ok(())
}

#[test]
fn an_expired_verb_is_refused() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let signer = signer.with_ttl(Duration::from_secs(10));
    let header = signer.sign_verb(RUN, b"{}", &scope("t1", "l1", None)?)?;
    let result = verifier.verify_verb(RUN, &header, b"{}", false, SystemTime::now() + Duration::from_secs(600));
    assert!(matches!(result, Err(DispatchAuthError::Expired)), "{result:?}");
    Ok(())
}

#[test]
fn a_verb_signed_for_a_clock_expires_after_its_lifetime_plus_skew() -> TestResult {
    let signer = DispatchSigner::new("k1", FORGE_SEED).with_ttl(Duration::from_secs(60));
    let verifier = forge_verifier(EnforcementMode::Require)?;
    let header = signer.sign_verb_at(RUN, b"{}", &scope("t1", "l1", None)?, at(1_000))?;
    let value: Value = serde_json::from_str(&header)?;
    assert_eq!(value["expires_at"], 1_060);
    verifier.verify_verb(RUN, &header, b"{}", false, at(1_090))?;
    let later = signer.sign_verb_at(RUN, b"{}", &scope("t1", "l1", None)?, at(1_000))?;
    let result = verifier.verify_verb(RUN, &later, b"{}", false, at(1_091));
    assert!(matches!(result, Err(DispatchAuthError::Expired)), "{result:?}");
    Ok(())
}

#[test]
fn a_verb_for_another_tenant_or_a_superseded_attempt_is_refused() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    check(&verifier, &dispatch_for(&signer, "t1", "a1")?)?;
    let body = b"{}";
    let foreign = signer.sign_verb(RUN, body, &scope("t2", "l1", Some("a1"))?)?;
    let result = verify(&verifier, RUN, &foreign, body);
    assert!(
        matches!(result, Err(DispatchAuthError::LeaseTenantConflict)),
        "{result:?}"
    );
    let older = signer.sign_verb(RUN, body, &scope("t1", "l1", Some("a0"))?)?;
    let result = verify(&verifier, RUN, &older, body);
    assert!(matches!(result, Err(DispatchAuthError::StaleAttempt)), "{result:?}");
    let live = signer.sign_verb(RUN, body, &scope("t1", "l1", Some("a1"))?)?;
    verify(&verifier, RUN, &live, body)?;
    Ok(())
}

#[test]
fn a_verb_for_a_lease_never_seen_is_admitted_without_recording_it() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let body = b"{}";
    let first = signer.sign_verb(RUN, body, &scope("t1", "l1", Some("a1"))?)?;
    verify(&verifier, RUN, &first, body)?;
    // The verb did not claim the lease for t1, so another tenant can still dispatch it.
    check(&verifier, &dispatch_for(&signer, "t2", "a1")?)?;
    Ok(())
}

#[test]
fn a_cleaned_attempt_cannot_be_dispatched_again() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let first = dispatch_for(&signer, "t1", "a1")?;
    check(&verifier, &first)?;
    // A second attempt while the first is live is refused.
    let result = check(&verifier, &dispatch_for(&signer, "t1", "a2")?);
    assert!(matches!(result, Err(DispatchAuthError::StaleAttempt)), "{result:?}");
    let body = cleanup_body("l1", "a1")?;
    let header = signer.sign_verb(CLEANUP, &body, &scope("t1", "l1", Some("a1"))?)?;
    verify(&verifier, CLEANUP, &header, &body)?;
    // The replayed dispatch of the cleaned attempt is refused, a new one is not.
    let result = check(&verifier, &first);
    assert!(matches!(result, Err(DispatchAuthError::StaleAttempt)), "{result:?}");
    check(&verifier, &dispatch_for(&signer, "t1", "a2")?)?;
    // The cleaned attempt's verbs are refused too, and the new attempt's are not.
    let stale = signer.sign_verb(RUN, b"{}", &scope("t1", "l1", Some("a1"))?)?;
    let result = verify(&verifier, RUN, &stale, b"{}");
    assert!(matches!(result, Err(DispatchAuthError::StaleAttempt)), "{result:?}");
    let live = signer.sign_verb(RUN, b"{}", &scope("t1", "l1", Some("a2"))?)?;
    verify(&verifier, RUN, &live, b"{}")?;
    Ok(())
}

#[test]
fn a_cleanup_of_a_superseded_attempt_is_refused_and_repeating_the_live_one_is_fine() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    check(&verifier, &dispatch_for(&signer, "t1", "a1")?)?;
    let a1 = cleanup_body("l1", "a1")?;
    for _ in 0..2 {
        let header = signer.sign_verb(CLEANUP, &a1, &scope("t1", "l1", Some("a1"))?)?;
        verify(&verifier, CLEANUP, &header, &a1)?;
    }
    check(&verifier, &dispatch_for(&signer, "t1", "a2")?)?;
    let header = signer.sign_verb(CLEANUP, &a1, &scope("t1", "l1", Some("a1"))?)?;
    let result = verify(&verifier, CLEANUP, &header, &a1);
    assert!(matches!(result, Err(DispatchAuthError::StaleAttempt)), "{result:?}");
    let foreign = signer.sign_verb(CLEANUP, &a1, &scope("t2", "l1", Some("a1"))?)?;
    let result = verify(&verifier, CLEANUP, &foreign, &a1);
    assert!(
        matches!(result, Err(DispatchAuthError::LeaseTenantConflict)),
        "{result:?}"
    );
    Ok(())
}

#[test]
fn a_cleanup_for_a_lease_never_dispatched_blocks_that_attempt() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let body = cleanup_body("l1", "a1")?;
    let header = signer.sign_verb(CLEANUP, &body, &scope("t1", "l1", Some("a1"))?)?;
    verify(&verifier, CLEANUP, &header, &body)?;
    let result = check(&verifier, &dispatch_for(&signer, "t1", "a1")?);
    assert!(matches!(result, Err(DispatchAuthError::StaleAttempt)), "{result:?}");
    check(&verifier, &dispatch_for(&signer, "t1", "a2")?)?;
    Ok(())
}

#[test]
fn a_cleanup_without_an_attempt_marks_the_lease_cleaned() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let body = serde_json::to_vec(&json!({"identity": {"lease_id": "l1", "provider": "mock"}}))?;
    let mut bare = scope("t1", "l1", None)?;
    bare.worker_id = None;
    for _ in 0..2 {
        let header = signer.sign_verb(CLEANUP, &body, &bare)?;
        verify(&verifier, CLEANUP, &header, &body)?;
    }
    Ok(())
}

#[test]
fn a_cleanup_must_match_its_signed_lease_and_attempt() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let body = cleanup_body("l1", "a1")?;
    let other_lease = signer.sign_verb(CLEANUP, &body, &scope("t1", "l2", Some("a1"))?)?;
    let result = verify(&verifier, CLEANUP, &other_lease, &body);
    assert!(matches!(result, Err(DispatchAuthError::LeaseMismatch)), "{result:?}");
    let other_attempt = signer.sign_verb(CLEANUP, &body, &scope("t1", "l1", Some("a2"))?)?;
    let result = verify(&verifier, CLEANUP, &other_attempt, &body);
    assert!(matches!(result, Err(DispatchAuthError::StaleAttempt)), "{result:?}");
    let junk = b"junk";
    let header = signer.sign_verb(CLEANUP, junk, &scope("t1", "l1", Some("a1"))?)?;
    let result = verify(&verifier, CLEANUP, &header, junk);
    assert!(
        matches!(
            result,
            Err(DispatchAuthError::Malformed(DispatchMalformedReason::Json(_)))
        ),
        "{result:?}"
    );
    Ok(())
}

#[test]
fn require_refuses_unsigned_requests_and_warn_accepts_them() -> TestResult {
    let (signer, require) = pair(EnforcementMode::Require)?;
    let warn = DispatchVerifier::new(EnforcementMode::Warn).with_key("k1", &signer.public_key_b64())?;
    let result = require.admit_message(RUN, None, b"{}");
    assert!(matches!(result, Err(DispatchAuthError::Missing)), "{result:?}");
    warn.admit_message(RUN, None, b"{}")?;
    // Subjects that are not signed requests are never checked.
    require.admit_message("sandbox.provider.firecracker.lease", None, b"{}")?;
    let headers = headers_for(&signer.sign_verb(RUN, b"{}", &scope("t1", "l1", None)?)?);
    require.admit_message(RUN, Some(&headers), b"{}")?;
    let result = require.admit_message(RUN, Some(&headers), b"{}");
    assert!(matches!(result, Err(DispatchAuthError::Replayed)), "{result:?}");
    // Headers without an envelope are as unsigned as no headers.
    let result = require.admit_message(RUN, Some(&HeaderMap::new()), b"{}");
    assert!(matches!(result, Err(DispatchAuthError::Missing)), "{result:?}");
    Ok(())
}

#[test]
fn off_never_looks_at_a_request_and_warn_without_keys_accepts_it_once_warned() -> TestResult {
    let off = DispatchVerifier::new(EnforcementMode::Off);
    off.admit_message(RUN, None, b"{}")?;
    let warn = DispatchVerifier::new(EnforcementMode::Warn);
    for _ in 0..2 {
        warn.admit_message(RUN, None, b"{}")?;
        assert!(warn.admit_message_scope(RUN, None, b"{}")?.is_none());
    }
    Ok(())
}

#[test]
fn the_fence_survives_a_restart() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    let signer = DispatchSigner::new("k1", random_seed());
    let build = || -> Result<DispatchVerifier, Box<dyn std::error::Error>> {
        Ok(DispatchVerifier::new(EnforcementMode::Require)
            .with_key("k1", &signer.public_key_b64())?
            .with_fence_file(&path)?)
    };
    let first = build()?;
    let original = dispatch_for(&signer, "t1", "a1")?;
    check(&first, &original)?;
    drop(first);

    let restarted = build()?;
    let result = check(&restarted, &dispatch_for(&signer, "t2", "a1")?);
    assert!(
        matches!(result, Err(DispatchAuthError::LeaseTenantConflict)),
        "{result:?}"
    );
    let result = check(&restarted, &dispatch_for(&signer, "t1", "a2")?);
    assert!(matches!(result, Err(DispatchAuthError::StaleAttempt)), "{result:?}");
    let body = cleanup_body("l1", "a1")?;
    let header = signer.sign_verb(CLEANUP, &body, &scope("t1", "l1", Some("a1"))?)?;
    verify(&restarted, CLEANUP, &header, &body)?;
    drop(restarted);

    let again = build()?;
    let result = check(&again, &original);
    assert!(matches!(result, Err(DispatchAuthError::StaleAttempt)), "{result:?}");
    let result = check(&again, &dispatch_for(&signer, "t2", "a1")?);
    assert!(
        matches!(result, Err(DispatchAuthError::LeaseTenantConflict)),
        "{result:?}"
    );
    again.forget_lease(&LeaseId::new("l1"));
    drop(again);
    check(&build()?, &dispatch_for(&signer, "t2", "a9")?)?;
    Ok(())
}

#[test]
fn a_torn_journal_line_is_ignored() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    std::fs::write(
        &path,
        "{\"l\":\"l1\",\"t\":\"t1\",\"a\":\"a1\",\"c\":false}\n{\"l\":\"l2\",\"t\"",
    )?;
    let signer = DispatchSigner::new("k1", random_seed());
    let verifier = DispatchVerifier::new(EnforcementMode::Require)
        .with_key("k1", &signer.public_key_b64())?
        .with_fence_file(&path)?;
    let result = check(&verifier, &dispatch_for(&signer, "t2", "a1")?);
    assert!(
        matches!(result, Err(DispatchAuthError::LeaseTenantConflict)),
        "{result:?}"
    );
    Ok(())
}

#[test]
fn unreadable_and_blank_journal_lines_are_skipped_and_tombstones_forget_a_lease() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    let mut content = Vec::new();
    content.extend_from_slice(b"\xff\xfe not utf8\n");
    content.extend_from_slice(b"not json\n");
    content.extend_from_slice(b"{\"l\":\"l1\",\"t\":\"t1\",\"a\":\"a1\",\"c\":false}\n");
    content.extend_from_slice(b"{\"l\":\"gone\",\"t\":\"t9\",\"a\":\"a1\",\"c\":false}\n");
    content.extend_from_slice(b"{\"l\":\"gone\",\"x\":true}\n");
    std::fs::write(&path, content)?;
    let signer = DispatchSigner::new("k1", random_seed());
    let verifier = DispatchVerifier::new(EnforcementMode::Require)
        .with_key("k1", &signer.public_key_b64())?
        .with_fence_file(&path)?;
    let result = check(&verifier, &dispatch_for(&signer, "t2", "a1")?);
    assert!(
        matches!(result, Err(DispatchAuthError::LeaseTenantConflict)),
        "{result:?}"
    );
    let forgotten = serde_json::to_vec(&json!({
        "lease_id": "gone", "request": {"tenant": "t2"},
        "target": {"worker_id": "w1", "attempt_id": "a1"},
    }))?;
    check(&verifier, &signer.sign_payload(&forgotten, OP_LEASE_DISPATCH)?)?;
    Ok(())
}

#[test]
fn a_journal_with_many_superseded_lines_is_compacted_on_open() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    let line = "{\"l\":\"l1\",\"t\":\"t1\",\"a\":\"a1\",\"c\":false}\n";
    std::fs::write(&path, line.repeat(1_100))?;
    let signer = DispatchSigner::new("k1", random_seed());
    let verifier = DispatchVerifier::new(EnforcementMode::Require)
        .with_key("k1", &signer.public_key_b64())?
        .with_fence_file(&path)?;
    assert_eq!(std::fs::read_to_string(&path)?.lines().count(), 1);
    let result = check(&verifier, &dispatch_for(&signer, "t2", "a1")?);
    assert!(
        matches!(result, Err(DispatchAuthError::LeaseTenantConflict)),
        "{result:?}"
    );
    Ok(())
}

#[test]
fn a_fence_file_in_a_missing_directory_is_created_with_its_parents() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("nested").join("deeper").join("fence.jsonl");
    let signer = DispatchSigner::new("k1", random_seed());
    let verifier = DispatchVerifier::new(EnforcementMode::Require)
        .with_key("k1", &signer.public_key_b64())?
        .with_fence_file(&path)?;
    check(&verifier, &dispatch_for(&signer, "t1", "a1")?)?;
    assert!(path.is_file());
    Ok(())
}

#[test]
fn a_fence_file_given_as_a_bare_name_resolves_in_the_working_directory_free_of_parents() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    let signer = DispatchSigner::new("k1", random_seed());
    let verifier = DispatchVerifier::new(EnforcementMode::Require)
        .with_key("k1", &signer.public_key_b64())?
        .with_fence_file(&path)?;
    check(&verifier, &dispatch_for(&signer, "t1", "a1")?)?;
    let json: Value = serde_json::from_str(std::fs::read_to_string(&path)?.trim())?;
    assert_eq!(json["l"], "l1");
    assert_eq!(json["t"], "t1");
    Ok(())
}
