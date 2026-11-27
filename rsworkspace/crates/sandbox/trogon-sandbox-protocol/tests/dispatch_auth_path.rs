//! Gateway, snapshot export, purge, owned cancellation and chunked-body paths.

use std::time::SystemTime;

use async_nats::HeaderMap;
use serde_json::{Value, json};
use trogon_sandbox_contract::{LeaseId, TenantId};
use trogon_sandbox_protocol::{
    AUTH_HEADER, DispatchAuthError, DispatchMalformedError, DispatchSigner, DispatchVerifier, EnforcementMode,
    OP_LEASE_DISPATCH, VerbScope,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ATTACH: &str = "sandbox.ssh.attach.l1";
const DETACH: &str = "sandbox.ssh.detach.l1";
const EXPORT: &str = "sandbox.worker.w1.snapshot.export";
const WRITE_FILES: &str = "sandbox.provider.mock.write_files.l1";
const CANCEL_OWNED: &str = "sandbox.provider.e2b.cancel_owned.l1";
const DELETE_SNAPSHOT: &str = "sandbox.provider.e2b.native_snapshot_delete";
const PURGE: &str = "sandbox.provider.e2b.purge_named";
const TRANSFER_HEADER: &str = "Sandbox-Transfer";

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

fn scope(tenant: &str, lease: &str) -> Result<VerbScope, Box<dyn std::error::Error>> {
    Ok(VerbScope {
        tenant: TenantId::new(tenant)?,
        lease_id: LeaseId::new(lease),
        worker_id: None,
        attempt_id: None,
        epoch: None,
    })
}

fn verify(verifier: &DispatchVerifier, subject: &str, header: &str, body: &[u8]) -> Result<(), DispatchAuthError> {
    verifier.verify_verb(subject, header, body, false, SystemTime::now())
}

fn headers_for(header: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(AUTH_HEADER, header);
    headers
}

/// Makes the verifier hold lease `l1` for `tenant`, as a signed dispatch would.
fn seen_lease(
    verifier: &DispatchVerifier,
    signer: &DispatchSigner,
    tenant: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let payload = serde_json::to_vec(&json!({
        "lease_id": "l1",
        "request": {"tenant": tenant},
        "target": {"worker_id": "w1", "attempt_id": "a1"},
    }))?;
    let mut value: Value = serde_json::from_slice(&signer.sign_payload(&payload, OP_LEASE_DISPATCH)?)?;
    Ok(verifier.verify_value(&mut value, OP_LEASE_DISPATCH, None, SystemTime::now())?)
}

#[test]
fn owned_lease_cancellation_is_signed_for_its_exact_lease_tenant_and_body() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let body = br#"{"tenant":"owner","lease_id":"l1","sandbox_id":"canonical-sandbox"}"#;
    let header = signer.sign_verb(CANCEL_OWNED, body, &scope("owner", "l1")?)?;
    verify(&verifier, CANCEL_OWNED, &header, body)?;
    let foreign = signer.sign_verb(CANCEL_OWNED, body, &scope("foreign", "l1")?)?;
    let result = verify(&verifier, CANCEL_OWNED, &foreign, body);
    assert!(matches!(result, Err(DispatchAuthError::TenantMismatch)), "{result:?}");
    let other = br#"{"tenant":"owner","lease_id":"l2","sandbox_id":"canonical-sandbox"}"#;
    let swapped = signer.sign_verb(CANCEL_OWNED, other, &scope("owner", "l1")?)?;
    let result = verify(&verifier, CANCEL_OWNED, &swapped, other);
    assert!(matches!(result, Err(DispatchAuthError::LeaseMismatch)), "{result:?}");
    Ok(())
}

#[test]
fn owned_lease_cancellation_needs_an_untransferred_well_formed_body() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let body = br#"{"tenant":"owner","lease_id":"l1","sandbox_id":"canonical-sandbox"}"#;
    let header = signer.sign_verb(CANCEL_OWNED, body, &scope("owner", "l1")?)?;
    let result = verifier.verify_verb(CANCEL_OWNED, &header, body, true, SystemTime::now());
    assert!(
        matches!(
            result,
            Err(DispatchAuthError::Malformed(
                DispatchMalformedError::OwnedCancellationTransfer
            ))
        ),
        "{result:?}"
    );
    let broken = br#"{"tenant":"owner"}"#;
    let header = signer.sign_verb(CANCEL_OWNED, broken, &scope("owner", "l1")?)?;
    let result = verify(&verifier, CANCEL_OWNED, &header, broken);
    assert!(
        matches!(
            result,
            Err(DispatchAuthError::Malformed(DispatchMalformedError::Json(_)))
        ),
        "{result:?}"
    );
    Ok(())
}

#[test]
fn native_snapshot_delete_is_signed_without_a_lease_and_binds_the_tenant_and_provider() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let body = br#"{"tenant":"owner","reference":{"provider":"e2b","id":"opaque-capture"}}"#;
    let header = signer.sign_verb(DELETE_SNAPSHOT, body, &scope("owner", "")?)?;
    verify(&verifier, DELETE_SNAPSHOT, &header, body)?;
    let result = verify(&verifier, DELETE_SNAPSHOT, &header, body);
    assert!(matches!(result, Err(DispatchAuthError::Replayed)), "{result:?}");
    let foreign = signer.sign_verb(DELETE_SNAPSHOT, body, &scope("other", "")?)?;
    let result = verify(&verifier, DELETE_SNAPSHOT, &foreign, body);
    assert!(matches!(result, Err(DispatchAuthError::TenantMismatch)), "{result:?}");
    let other_provider = "sandbox.provider.daytona.native_snapshot_delete";
    let header = signer.sign_verb(other_provider, body, &scope("owner", "")?)?;
    let result = verify(&verifier, other_provider, &header, body);
    assert!(
        matches!(
            result,
            Err(DispatchAuthError::Malformed(
                DispatchMalformedError::NativeSnapshotProvider
            ))
        ),
        "{result:?}"
    );
    let lease_scoped = signer.sign_verb(DELETE_SNAPSHOT, body, &scope("owner", "invented-lease")?)?;
    let result = verify(&verifier, DELETE_SNAPSHOT, &lease_scoped, body);
    assert!(matches!(result, Err(DispatchAuthError::LeaseMismatch)), "{result:?}");
    Ok(())
}

#[test]
fn native_snapshot_delete_refuses_a_transferred_or_malformed_body_and_extra_bindings() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let body = br#"{"tenant":"owner","reference":{"provider":"e2b","id":"opaque-capture"}}"#;
    let header = signer.sign_verb(DELETE_SNAPSHOT, body, &scope("owner", "")?)?;
    let result = verifier.verify_verb(DELETE_SNAPSHOT, &header, body, true, SystemTime::now());
    assert!(
        matches!(
            result,
            Err(DispatchAuthError::Malformed(
                DispatchMalformedError::NativeSnapshotDeleteTransfer
            ))
        ),
        "{result:?}"
    );
    let junk = b"[]";
    let header = signer.sign_verb(DELETE_SNAPSHOT, junk, &scope("owner", "")?)?;
    let result = verify(&verifier, DELETE_SNAPSHOT, &header, junk);
    assert!(
        matches!(
            result,
            Err(DispatchAuthError::Malformed(DispatchMalformedError::Json(_)))
        ),
        "{result:?}"
    );
    let mut with_worker = scope("owner", "")?;
    with_worker.worker_id = Some(trogon_sandbox_contract::WorkerId::new("w1")?);
    let header = signer.sign_verb(DELETE_SNAPSHOT, body, &with_worker)?;
    let result = verify(&verifier, DELETE_SNAPSHOT, &header, body);
    assert!(matches!(result, Err(DispatchAuthError::LeaseMismatch)), "{result:?}");
    Ok(())
}

#[test]
fn ssh_and_export_subjects_are_classified_as_signed_requests() -> TestResult {
    let (_, verifier) = pair(EnforcementMode::Require)?;
    // A request on one of these subjects is demanded to be signed; an unrelated subject is not.
    for subject in [ATTACH, DETACH, EXPORT, PURGE, CANCEL_OWNED, DELETE_SNAPSHOT] {
        let result = verifier.admit_message(subject, None, b"{}");
        assert!(
            matches!(result, Err(DispatchAuthError::Missing)),
            "{subject}: {result:?}"
        );
    }
    for other in [
        "sandbox.ssh.attach",
        "sandbox.ssh.stream.s1.to_worker",
        "sandbox.worker.w1.snapshot.get",
    ] {
        verifier.admit_message(other, None, b"{}")?;
    }
    Ok(())
}

#[test]
fn a_signed_ssh_attach_and_detach_verify_and_bind_the_body_lease() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let body = br#"{"session_id":"s1","lease_id":"l1"}"#;
    for subject in [ATTACH, DETACH] {
        let header = signer.sign_verb(subject, body, &scope("t1", "l1")?)?;
        verify(&verifier, subject, &header, body)?;
    }
    let other = br#"{"session_id":"s1","lease_id":"l2"}"#;
    let header = signer.sign_verb(ATTACH, other, &scope("t1", "l1")?)?;
    let result = verify(&verifier, ATTACH, &header, other);
    assert!(matches!(result, Err(DispatchAuthError::LeaseMismatch)), "{result:?}");
    let signed_for_other = signer.sign_verb(ATTACH, body, &scope("t1", "l2")?)?;
    let result = verify(&verifier, ATTACH, &signed_for_other, body);
    assert!(matches!(result, Err(DispatchAuthError::LeaseMismatch)), "{result:?}");
    let not_json = b"nope";
    let header = signer.sign_verb(ATTACH, not_json, &scope("t1", "l1")?)?;
    let result = verify(&verifier, ATTACH, &header, not_json);
    assert!(
        matches!(
            result,
            Err(DispatchAuthError::Malformed(DispatchMalformedError::Json(_)))
        ),
        "{result:?}"
    );
    Ok(())
}

#[test]
fn ssh_for_another_tenants_lease_is_refused() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    seen_lease(&verifier, &signer, "t1")?;
    let body = br#"{"session_id":"s1","lease_id":"l1"}"#;
    for subject in [ATTACH, DETACH] {
        let foreign = signer.sign_verb(subject, body, &scope("t2", "l1")?)?;
        let result = verify(&verifier, subject, &foreign, body);
        assert!(
            matches!(result, Err(DispatchAuthError::LeaseTenantConflict)),
            "{result:?}"
        );
        let own = signer.sign_verb(subject, body, &scope("t1", "l1")?)?;
        verify(&verifier, subject, &own, body)?;
    }
    Ok(())
}

#[test]
fn unsigned_gateway_and_export_requests_follow_the_mode() -> TestResult {
    let (signer, require) = pair(EnforcementMode::Require)?;
    let warn = DispatchVerifier::new(EnforcementMode::Warn).with_key("k1", &signer.public_key_b64())?;
    let body = br#"{"session_id":"s1","lease_id":"l1"}"#;
    for subject in [ATTACH, DETACH, EXPORT] {
        let result = require.admit_message(subject, None, body);
        assert!(matches!(result, Err(DispatchAuthError::Missing)), "{result:?}");
        warn.admit_message(subject, None, body)?;
    }
    // Warn logs and accepts a signed request for the wrong tenant.
    seen_lease(&warn, &signer, "t1")?;
    let foreign = signer.sign_verb(ATTACH, body, &scope("t2", "l1")?)?;
    warn.admit_message(ATTACH, Some(&headers_for(&foreign)), body)?;
    // Require refuses it.
    seen_lease(&require, &signer, "t1")?;
    let foreign = signer.sign_verb(ATTACH, body, &scope("t2", "l1")?)?;
    let result = require.admit_message(ATTACH, Some(&headers_for(&foreign)), body);
    assert!(
        matches!(result, Err(DispatchAuthError::LeaseTenantConflict)),
        "{result:?}"
    );
    Ok(())
}

#[test]
fn a_snapshot_export_binds_its_lease_and_tenant() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    seen_lease(&verifier, &signer, "t1")?;
    let body = br#"{"lease_id":"l1","target":{"registry":"r"}}"#;
    let foreign = signer.sign_verb(EXPORT, body, &scope("t2", "l1")?)?;
    let result = verify(&verifier, EXPORT, &foreign, body);
    assert!(
        matches!(result, Err(DispatchAuthError::LeaseTenantConflict)),
        "{result:?}"
    );
    let swapped = signer.sign_verb(EXPORT, body, &scope("t1", "l9")?)?;
    let result = verify(&verifier, EXPORT, &swapped, body);
    assert!(matches!(result, Err(DispatchAuthError::LeaseMismatch)), "{result:?}");
    let own = signer.sign_verb(EXPORT, body, &scope("t1", "l1")?)?;
    verify(&verifier, EXPORT, &own, body)?;
    let result = verify(&verifier, EXPORT, &own, body);
    assert!(matches!(result, Err(DispatchAuthError::Replayed)), "{result:?}");
    Ok(())
}

#[test]
fn a_transferred_export_or_gateway_request_skips_only_the_body_comparison() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    for (subject, lease) in [(EXPORT, "l1"), (ATTACH, "l1")] {
        let full = br#"{"lease_id":"l1"}"#;
        let header = signer.sign_verb(subject, full, &scope("t1", lease)?)?;
        verifier.verify_verb(subject, &header, br#"{"inbox":"x"}"#, true, SystemTime::now())?;
    }
    Ok(())
}

#[test]
fn a_purge_binds_the_tenant_in_its_body() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let body = br#"{"tenant":"owner","names":["a"]}"#;
    let header = signer.sign_verb(PURGE, body, &scope("owner", "")?)?;
    verify(&verifier, PURGE, &header, body)?;
    let foreign = signer.sign_verb(PURGE, body, &scope("other", "")?)?;
    let result = verify(&verifier, PURGE, &foreign, body);
    assert!(matches!(result, Err(DispatchAuthError::TenantMismatch)), "{result:?}");
    let junk = b"junk";
    let header = signer.sign_verb(PURGE, junk, &scope("owner", "")?)?;
    let result = verify(&verifier, PURGE, &header, junk);
    assert!(
        matches!(
            result,
            Err(DispatchAuthError::Malformed(DispatchMalformedError::Json(_)))
        ),
        "{result:?}"
    );
    let chunked = signer.sign_verb(PURGE, body, &scope("owner", "")?)?;
    verifier.verify_verb(PURGE, &chunked, b"descriptor", true, SystemTime::now())?;
    Ok(())
}

#[test]
fn a_lease_never_seen_is_admitted_for_a_signed_tenant_only() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let body = br#"{"session_id":"s1","lease_id":"l1"}"#;
    // The tenant is a typed value, so a request for no tenant cannot even be built.
    assert!(scope("", "l1").is_err());
    let named = signer.sign_verb(ATTACH, body, &scope("t1", "l1")?)?;
    verify(&verifier, ATTACH, &named, body)?;
    let mut anonymous: Value = serde_json::from_str(&named)?;
    anonymous["tenant"] = "".into();
    let result = verify(&verifier, ATTACH, &anonymous.to_string(), body);
    assert!(
        matches!(
            result,
            Err(DispatchAuthError::Malformed(DispatchMalformedError::Json(_)))
        ),
        "{result:?}"
    );
    let result = verifier.admit_message(ATTACH, None, body);
    assert!(matches!(result, Err(DispatchAuthError::Missing)), "{result:?}");
    Ok(())
}

#[test]
fn a_reassembled_body_must_match_the_signed_digest() -> TestResult {
    let (signer, require) = pair(EnforcementMode::Require)?;
    let warn = DispatchVerifier::new(EnforcementMode::Warn).with_key("k1", &signer.public_key_b64())?;
    let full = br#"{"lease_id":"l1","files":["a","b"]}"#;
    let header = signer.sign_verb(WRITE_FILES, full, &scope("t1", "l1")?)?;
    let headers = headers_for(&header);
    require.check_reassembled(&headers, full)?;
    let result = require.check_reassembled(&headers, b"tampered");
    assert!(matches!(result, Err(DispatchAuthError::BodyMismatch)), "{result:?}");
    warn.check_reassembled(&headers, b"tampered")?;
    // No envelope: admission already decided, nothing to compare.
    require.check_reassembled(&HeaderMap::new(), b"x")?;
    Ok(())
}

#[test]
fn a_reassembled_body_with_an_unreadable_envelope_follows_the_mode() -> TestResult {
    let (signer, require) = pair(EnforcementMode::Require)?;
    let warn = DispatchVerifier::new(EnforcementMode::Warn).with_key("k1", &signer.public_key_b64())?;
    let headers = headers_for("not json");
    let result = require.check_reassembled(&headers, b"x");
    assert!(
        matches!(
            result,
            Err(DispatchAuthError::Malformed(DispatchMalformedError::Json(_)))
        ),
        "{result:?}"
    );
    warn.check_reassembled(&headers, b"x")?;
    Ok(())
}

#[test]
fn a_reassembled_body_is_not_checked_when_nothing_is_enforced() -> TestResult {
    let (signer, _) = pair(EnforcementMode::Off)?;
    let header = signer.sign_verb(WRITE_FILES, b"{}", &scope("t1", "l1")?)?;
    let headers = headers_for(&header);
    DispatchVerifier::new(EnforcementMode::Off).check_reassembled(&headers, b"different")?;
    DispatchVerifier::new(EnforcementMode::Warn).check_reassembled(&headers, b"different")?;
    Ok(())
}

#[test]
fn a_chunked_request_skips_only_the_descriptor_digest() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let full = br#"{"lease_id":"l1"}"#;
    let header = signer.sign_verb(WRITE_FILES, full, &scope("t1", "l1")?)?;
    verifier.verify_verb(WRITE_FILES, &header, br#"{"inbox":"x"}"#, true, SystemTime::now())?;
    let wrong_lease = signer.sign_verb(WRITE_FILES, full, &scope("t1", "l2")?)?;
    let result = verifier.verify_verb(WRITE_FILES, &wrong_lease, b"{}", true, SystemTime::now());
    assert!(matches!(result, Err(DispatchAuthError::LeaseMismatch)), "{result:?}");
    Ok(())
}

#[test]
fn the_transfer_header_marks_an_admitted_message_as_a_descriptor() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let full = br#"{"lease_id":"l1","files":[]}"#;
    let header = signer.sign_verb(WRITE_FILES, full, &scope("t1", "l1")?)?;
    let mut headers = headers_for(&header);
    headers.insert(TRANSFER_HEADER, "descriptor");
    verifier.admit_message(WRITE_FILES, Some(&headers), br#"{"inbox":"x"}"#)?;
    let header = signer.sign_verb(WRITE_FILES, full, &scope("t1", "l1")?)?;
    let result = verifier.admit_message(WRITE_FILES, Some(&headers_for(&header)), br#"{"inbox":"x"}"#);
    assert!(matches!(result, Err(DispatchAuthError::BodyMismatch)), "{result:?}");
    Ok(())
}
