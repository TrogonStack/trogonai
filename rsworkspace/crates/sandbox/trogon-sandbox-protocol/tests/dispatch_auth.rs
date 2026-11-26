//! Signing and verifying lease dispatches, the configuration readers, and the error values.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signer as _, SigningKey};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use trogon_sandbox_contract::{DispatchAuth, FileSha256, LeaseEpoch, LeaseId, TenantId, WorkerId};
use trogon_sandbox_protocol::{
    AUTH_FIELD, DispatchAuthError, DispatchDecodeError, DispatchMalformedReason, DispatchPublicKeyError,
    DispatchSignError, DispatchSigner, DispatchSignerConfigError, DispatchVerifier, DispatchVerifierConfigError,
    ENV_FENCE_EPOCH, ENV_FENCE_FILE, ENV_PUBLIC_KEYS, ENV_PUBLIC_KEYS_FILE, ENV_SIGNING_KEY, ENV_SIGNING_KEY_FILE,
    ENV_SIGNING_KEY_ID, ENV_SIGNING_TTL_SECS, ENV_VERIFY_MODE, EnforcementMode, EnforcementModeError, EpochMode,
    EpochModeError, FreshLaunchFenceError, OP_LEASE_DISPATCH,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn random_seed() -> [u8; 32] {
    let mut seed = [0u8; 32];
    seed[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    seed[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    seed
}

fn dispatch(tenant: &str, lease: &str) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&json!({
        "lease_id": lease,
        "request": {"tenant": tenant, "note": "x"},
        "target": {"worker_id": "w1", "attempt_id": "a1"},
    }))
}

fn pair(mode: EnforcementMode) -> Result<(DispatchSigner, DispatchVerifier), Box<dyn std::error::Error>> {
    let signer = DispatchSigner::new("k1", random_seed());
    let verifier = DispatchVerifier::new(mode).with_key("k1", &signer.public_key_b64())?;
    Ok((signer, verifier))
}

fn check(verifier: &DispatchVerifier, payload: &[u8], now: SystemTime) -> Result<(), DispatchAuthError> {
    let mut value: Value = serde_json::from_slice(payload).map_err(DispatchMalformedReason::Json)?;
    verifier.verify_value(&mut value, OP_LEASE_DISPATCH, None, now)
}

fn sign(signer: &DispatchSigner, tenant: &str, lease: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(signer.sign_payload(&dispatch(tenant, lease)?, OP_LEASE_DISPATCH)?)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// The bytes an envelope is signed over, written out independently of the library.
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

const FORGE_SEED: [u8; 32] = [9u8; 32];

fn forge_verifier(mode: EnforcementMode) -> Result<DispatchVerifier, Box<dyn std::error::Error>> {
    let signer = DispatchSigner::new("k1", FORGE_SEED);
    Ok(DispatchVerifier::new(mode).with_key("k1", &signer.public_key_b64())?)
}

/// A dispatch whose envelope is built and signed by hand, so each field can be set to a chosen value.
fn forged(mutate: impl FnOnce(&mut DispatchAuth)) -> Result<Value, Box<dyn std::error::Error>> {
    let body = json!({
        "lease_id": "l1",
        "request": {"tenant": "t1", "note": "x"},
        "target": {"worker_id": "w1", "attempt_id": "a1"},
    });
    let digest = hex(&Sha256::digest(serde_json::to_vec(&body)?));
    let mut auth = DispatchAuth {
        version: 1,
        key_id: "k1".to_owned(),
        operation: OP_LEASE_DISPATCH.to_owned(),
        tenant: TenantId::new("t1")?,
        lease_id: LeaseId::new("l1"),
        worker_id: None,
        attempt_id: None,
        nonce: None,
        epoch: None,
        expires_at: 1_000_000,
        body_sha256: FileSha256::new(digest)?,
        signature: String::new(),
    };
    mutate(&mut auth);
    auth.signature = B64.encode(
        SigningKey::from_bytes(&FORGE_SEED)
            .sign(&signing_input(&auth))
            .to_bytes(),
    );
    let mut value = body;
    value[AUTH_FIELD] = serde_json::to_value(&auth)?;
    Ok(value)
}

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

fn signer_from(env: &HashMap<String, String>) -> Result<Option<DispatchSigner>, DispatchSignerConfigError> {
    DispatchSigner::from_lookup(&|name| env.get(name).cloned())
}

fn verifier_from(
    env: &HashMap<String, String>,
    default_fence: Option<std::path::PathBuf>,
) -> Result<DispatchVerifier, DispatchVerifierConfigError> {
    DispatchVerifier::from_lookup(&|name| env.get(name).cloned(), default_fence)
}

#[test]
fn sign_and_verify_round_trip() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let signed = sign(&signer, "t1", "l1")?;
    check(&verifier, &signed, SystemTime::now())?;
    let own = WorkerId::new("w1")?;
    let mut value: Value = serde_json::from_slice(&signed)?;
    verifier.verify_value(&mut value, OP_LEASE_DISPATCH, Some(&own), SystemTime::now())?;
    assert!(value.get(AUTH_FIELD).is_none());
    Ok(())
}

#[test]
fn the_envelope_is_signed_over_the_documented_input() -> TestResult {
    let signer = DispatchSigner::new("k1", FORGE_SEED);
    let signed = signer.sign_payload_at(&dispatch("t1", "l1")?, OP_LEASE_DISPATCH, at(1_000))?;
    let value: Value = serde_json::from_slice(&signed)?;
    let auth: DispatchAuth = serde_json::from_value(value[AUTH_FIELD].clone())?;
    assert_eq!(auth.version, 1);
    assert_eq!(auth.key_id, "k1");
    assert_eq!(auth.operation, OP_LEASE_DISPATCH);
    assert_eq!(auth.expires_at, 1_300);
    assert!(auth.nonce.is_none());
    let public = ed25519_dalek::VerifyingKey::from_bytes(
        &<[u8; 32]>::try_from(B64.decode(signer.public_key_b64())?).map_err(|_| "key is not 32 bytes")?,
    )?;
    let signature = ed25519_dalek::Signature::from_slice(&B64.decode(&auth.signature)?)?;
    ed25519_dalek::Verifier::verify(&public, &signing_input(&auth), &signature)?;
    Ok(())
}

#[test]
fn an_epoch_and_nonce_are_part_of_what_is_signed() -> TestResult {
    let verifier = forge_verifier(EnforcementMode::Require)?;
    let mut value = forged(|auth| {
        auth.epoch = LeaseEpoch::new(7).ok();
        auth.nonce = Some("n1".to_owned());
    })?;
    verifier.verify_value(&mut value, OP_LEASE_DISPATCH, None, at(1_000))?;
    for (field, replacement) in [("epoch", json!(8)), ("nonce", json!("n2"))] {
        let mut value = forged(|auth| {
            auth.epoch = LeaseEpoch::new(7).ok();
            auth.nonce = Some("n1".to_owned());
        })?;
        value[AUTH_FIELD][field] = replacement;
        let result = verifier.verify_value(&mut value, OP_LEASE_DISPATCH, None, at(1_000));
        assert!(
            matches!(result, Err(DispatchAuthError::BadSignature)),
            "{field}: {result:?}"
        );
    }
    Ok(())
}

#[test]
fn a_tampered_field_fails() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let signed = sign(&signer, "t1", "l1")?;
    let mut value: Value = serde_json::from_slice(&signed)?;
    value["request"]["note"] = "y".into();
    let result = check(&verifier, &serde_json::to_vec(&value)?, SystemTime::now());
    assert!(matches!(result, Err(DispatchAuthError::BodyMismatch)), "{result:?}");
    let mut value: Value = serde_json::from_slice(&signed)?;
    value[AUTH_FIELD]["tenant"] = "t2".into();
    let result = check(&verifier, &serde_json::to_vec(&value)?, SystemTime::now());
    assert!(matches!(result, Err(DispatchAuthError::BadSignature)), "{result:?}");
    let mut value: Value = serde_json::from_slice(&signed)?;
    value[AUTH_FIELD]["expires_at"] = u64::MAX.into();
    let result = check(&verifier, &serde_json::to_vec(&value)?, SystemTime::now());
    assert!(matches!(result, Err(DispatchAuthError::BadSignature)), "{result:?}");
    Ok(())
}

#[test]
fn an_expired_envelope_fails() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let signer = signer.with_ttl(Duration::from_secs(10));
    let signed = sign(&signer, "t1", "l1")?;
    let later = SystemTime::now() + Duration::from_secs(600);
    let result = check(&verifier, &signed, later);
    assert!(matches!(result, Err(DispatchAuthError::Expired)), "{result:?}");
    Ok(())
}

#[test]
fn expiry_allows_thirty_seconds_of_clock_skew() -> TestResult {
    let verifier = forge_verifier(EnforcementMode::Require)?;
    let mut value = forged(|_| {})?;
    verifier.verify_value(&mut value, OP_LEASE_DISPATCH, None, at(1_000_030))?;
    let mut value = forged(|_| {})?;
    let result = verifier.verify_value(&mut value, OP_LEASE_DISPATCH, None, at(1_000_031));
    assert!(matches!(result, Err(DispatchAuthError::Expired)), "{result:?}");
    let mut value = forged(|_| {})?;
    verifier.verify_value(&mut value, OP_LEASE_DISPATCH, None, at(0))?;
    Ok(())
}

#[test]
fn signing_times_are_taken_from_the_given_clock_and_ttl() -> TestResult {
    let signer = DispatchSigner::new("k1", FORGE_SEED).with_ttl(Duration::from_secs(40));
    let signed = signer.sign_payload_at(&dispatch("t1", "l1")?, OP_LEASE_DISPATCH, at(5_000))?;
    let value: Value = serde_json::from_slice(&signed)?;
    assert_eq!(value[AUTH_FIELD]["expires_at"], 5_040);
    assert_eq!(signer.key_id(), "k1");
    Ok(())
}

#[test]
fn a_wrong_operation_worker_or_key_fails() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let signed = signer.sign_payload(&dispatch("t1", "l1")?, "other")?;
    let result = check(&verifier, &signed, SystemTime::now());
    assert!(
        matches!(&result, Err(DispatchAuthError::WrongOperation { expected, found })
            if expected == OP_LEASE_DISPATCH && found == "other"),
        "{result:?}"
    );
    let signed = sign(&signer, "t1", "l1")?;
    let other = WorkerId::new("w2")?;
    let mut value: Value = serde_json::from_slice(&signed)?;
    let result = verifier.verify_value(&mut value, OP_LEASE_DISPATCH, Some(&other), SystemTime::now());
    assert!(
        matches!(&result, Err(DispatchAuthError::WrongWorker { expected, found })
            if expected == "w2" && found == "w1"),
        "{result:?}"
    );
    let stranger = DispatchSigner::new("k1", random_seed());
    let result = check(&verifier, &sign(&stranger, "t1", "l1")?, SystemTime::now());
    assert!(matches!(result, Err(DispatchAuthError::BadSignature)), "{result:?}");
    let unknown = DispatchSigner::new("k9", random_seed());
    let result = check(&verifier, &sign(&unknown, "t1", "l1")?, SystemTime::now());
    assert!(
        matches!(&result, Err(DispatchAuthError::UnknownKey(id)) if id == "k9"),
        "{result:?}"
    );
    Ok(())
}

#[test]
fn a_dispatch_without_a_worker_is_accepted_by_any_worker() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let payload = serde_json::to_vec(&json!({"lease_id": "l1", "request": {"tenant": "t1"}}))?;
    let signed = signer.sign_payload(&payload, OP_LEASE_DISPATCH)?;
    let own = WorkerId::new("w2")?;
    let mut value: Value = serde_json::from_slice(&signed)?;
    verifier.verify_value(&mut value, OP_LEASE_DISPATCH, Some(&own), SystemTime::now())?;
    Ok(())
}

#[test]
fn a_reused_lease_id_for_another_tenant_is_refused() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let first = sign(&signer, "t1", "l1")?;
    let again = sign(&signer, "t1", "l1")?;
    let other = sign(&signer, "t2", "l1")?;
    check(&verifier, &first, SystemTime::now())?;
    check(&verifier, &again, SystemTime::now())?;
    let result = check(&verifier, &other, SystemTime::now());
    assert!(
        matches!(result, Err(DispatchAuthError::LeaseTenantConflict)),
        "{result:?}"
    );
    verifier.forget_lease(&LeaseId::new("l1"));
    check(&verifier, &other, SystemTime::now())?;
    Ok(())
}

#[test]
fn forgetting_an_unknown_lease_changes_nothing() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    check(&verifier, &sign(&signer, "t1", "l1")?, SystemTime::now())?;
    verifier.forget_lease(&LeaseId::new("never-seen"));
    let result = check(&verifier, &sign(&signer, "t2", "l1")?, SystemTime::now());
    assert!(
        matches!(result, Err(DispatchAuthError::LeaseTenantConflict)),
        "{result:?}"
    );
    Ok(())
}

#[test]
fn a_second_worker_cannot_take_a_live_attempt() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let body = |worker: &str| {
        serde_json::to_vec(&json!({
            "lease_id": "l1",
            "request": {"tenant": "t1"},
            "target": {"worker_id": worker, "attempt_id": "a1"},
        }))
    };
    let first = signer.sign_payload(&body("w1")?, OP_LEASE_DISPATCH)?;
    check(&verifier, &first, SystemTime::now())?;
    let second = signer.sign_payload(&body("w2")?, OP_LEASE_DISPATCH)?;
    let result = check(&verifier, &second, SystemTime::now());
    assert!(
        matches!(&result, Err(DispatchAuthError::WrongWorker { expected, found })
            if expected == "w1" && found == "w2"),
        "{result:?}"
    );
    Ok(())
}

#[test]
fn warn_accepts_and_require_refuses_missing_or_bad_signatures() -> TestResult {
    let (signer, warn) = pair(EnforcementMode::Warn)?;
    let require = DispatchVerifier::new(EnforcementMode::Require).with_key("k1", &signer.public_key_b64())?;
    let unsigned = dispatch("t1", "l1")?;
    let mut bad: Value = serde_json::from_slice(&sign(&signer, "t1", "l1")?)?;
    bad["request"]["note"] = "tampered".into();
    let bad = serde_json::to_vec(&bad)?;
    for payload in [&unsigned, &bad] {
        let accepted: Value = warn.decode(payload, OP_LEASE_DISPATCH, None)?;
        assert!(accepted.get(AUTH_FIELD).is_none());
        assert!(matches!(
            require.decode::<Value>(payload, OP_LEASE_DISPATCH, None),
            Err(DispatchDecodeError::Auth(_))
        ));
    }
    let signed = sign(&signer, "t1", "l1")?;
    let decoded: Value = require.decode(&signed, OP_LEASE_DISPATCH, None)?;
    assert_eq!(decoded["request"]["tenant"], "t1");
    assert!(decoded.get(AUTH_FIELD).is_none());
    Ok(())
}

#[test]
fn warn_with_no_keys_and_off_accept_unsigned_dispatches() -> TestResult {
    for mode in [EnforcementMode::Warn, EnforcementMode::Off] {
        let verifier = DispatchVerifier::new(mode);
        assert_eq!(verifier.mode(), mode);
        for _ in 0..2 {
            let decoded: Value = verifier.decode(&dispatch("t", "l")?, OP_LEASE_DISPATCH, None)?;
            assert_eq!(decoded["lease_id"], "l");
        }
    }
    Ok(())
}

#[test]
fn off_strips_a_present_envelope_without_checking_it() -> TestResult {
    let (signer, _) = pair(EnforcementMode::Off)?;
    let verifier = DispatchVerifier::new(EnforcementMode::Off);
    let mut tampered: Value = serde_json::from_slice(&sign(&signer, "t1", "l1")?)?;
    tampered["request"]["note"] = "changed".into();
    let decoded: Value = verifier.decode(&serde_json::to_vec(&tampered)?, OP_LEASE_DISPATCH, None)?;
    assert!(decoded.get(AUTH_FIELD).is_none());
    assert_eq!(decoded["request"]["note"], "changed");
    Ok(())
}

#[test]
fn decode_reports_json_and_shape_errors() -> TestResult {
    let verifier = DispatchVerifier::new(EnforcementMode::Off);
    assert!(matches!(
        verifier.decode::<Value>(b"not json", OP_LEASE_DISPATCH, None),
        Err(DispatchDecodeError::Json(_))
    ));
    assert!(matches!(
        verifier.decode::<u32>(b"\"text\"", OP_LEASE_DISPATCH, None),
        Err(DispatchDecodeError::Json(_))
    ));
    let (_, require) = pair(EnforcementMode::Require)?;
    let result = require.decode::<Value>(b"[1]", OP_LEASE_DISPATCH, None);
    assert!(
        matches!(
            result,
            Err(DispatchDecodeError::Auth(DispatchAuthError::Malformed(
                DispatchMalformedReason::NotAnObject
            )))
        ),
        "{result:?}"
    );
    Ok(())
}

#[test]
fn a_missing_or_unreadable_envelope_is_refused() -> TestResult {
    let (_, verifier) = pair(EnforcementMode::Require)?;
    let mut missing: Value = serde_json::from_slice(&dispatch("t1", "l1")?)?;
    let result = verifier.verify_value(&mut missing, OP_LEASE_DISPATCH, None, SystemTime::now());
    assert!(matches!(result, Err(DispatchAuthError::Missing)), "{result:?}");
    let mut not_object = json!([1, 2]);
    let result = verifier.verify_value(&mut not_object, OP_LEASE_DISPATCH, None, SystemTime::now());
    assert!(
        matches!(
            result,
            Err(DispatchAuthError::Malformed(DispatchMalformedReason::NotAnObject))
        ),
        "{result:?}"
    );
    let mut garbled = json!({"dispatch_auth": "text"});
    let result = verifier.verify_value(&mut garbled, OP_LEASE_DISPATCH, None, SystemTime::now());
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
fn an_unsupported_version_or_malformed_signature_is_refused() -> TestResult {
    let verifier = forge_verifier(EnforcementMode::Require)?;
    let mut value = forged(|auth| auth.version = 2)?;
    let result = verifier.verify_value(&mut value, OP_LEASE_DISPATCH, None, at(1_000));
    assert!(
        matches!(
            result,
            Err(DispatchAuthError::Malformed(
                DispatchMalformedReason::UnsupportedVersion { version: 2 }
            ))
        ),
        "{result:?}"
    );
    for signature in ["not base64 !!", "AAAA", ""] {
        let mut value = forged(|_| {})?;
        value[AUTH_FIELD]["signature"] = signature.into();
        let result = verifier.verify_value(&mut value, OP_LEASE_DISPATCH, None, at(1_000));
        assert!(
            matches!(
                result,
                Err(DispatchAuthError::Malformed(DispatchMalformedReason::Signature))
            ),
            "{signature}: {result:?}"
        );
    }
    Ok(())
}

#[test]
fn a_hand_signed_envelope_verifies() -> TestResult {
    let verifier = forge_verifier(EnforcementMode::Require)?;
    let mut value = forged(|_| {})?;
    verifier.verify_value(&mut value, OP_LEASE_DISPATCH, None, at(1_000))?;
    Ok(())
}

#[test]
fn an_envelope_that_disagrees_with_the_body_is_refused() -> TestResult {
    let verifier = forge_verifier(EnforcementMode::Require)?;
    let mut tenant = forged(|auth| auth.tenant = TenantId::new("other").unwrap_or_else(|_| auth.tenant.clone()))?;
    let result = verifier.verify_value(&mut tenant, OP_LEASE_DISPATCH, None, at(1_000));
    assert!(matches!(result, Err(DispatchAuthError::TenantMismatch)), "{result:?}");
    let mut lease = forged(|auth| auth.lease_id = LeaseId::new("l2"))?;
    let result = verifier.verify_value(&mut lease, OP_LEASE_DISPATCH, None, at(1_000));
    assert!(matches!(result, Err(DispatchAuthError::TenantMismatch)), "{result:?}");
    let zeros = FileSha256::new("0".repeat(64))?;
    let mut digest = forged(|auth| auth.body_sha256 = zeros)?;
    let result = verifier.verify_value(&mut digest, OP_LEASE_DISPATCH, None, at(1_000));
    assert!(matches!(result, Err(DispatchAuthError::BodyMismatch)), "{result:?}");
    Ok(())
}

#[test]
fn enforcement_modes_parse_case_and_space_insensitively() -> TestResult {
    assert_eq!(EnforcementMode::parse("Require")?, EnforcementMode::Require);
    assert_eq!(EnforcementMode::parse(" OFF ")?, EnforcementMode::Off);
    assert_eq!(EnforcementMode::parse("warn")?, EnforcementMode::Warn);
    assert_eq!(EnforcementMode::parse("")?, EnforcementMode::Warn);
    assert_eq!("require".parse::<EnforcementMode>()?, EnforcementMode::Require);
    let error = EnforcementMode::parse("Strict").err().ok_or("strict must be refused")?;
    assert_eq!(error.found, "strict");
    assert_eq!(
        error.to_string(),
        "SANDBOX_DISPATCH_VERIFY must be off, warn or require, got strict"
    );
    assert_eq!(
        "bogus".parse::<EnforcementMode>().err(),
        Some(EnforcementModeError {
            found: "bogus".to_owned()
        })
    );
    Ok(())
}

#[test]
fn epoch_modes_parse_case_and_space_insensitively() -> TestResult {
    assert_eq!(EpochMode::parse("Require")?, EpochMode::Require);
    assert_eq!(EpochMode::parse(" warn ")?, EpochMode::Warn);
    assert_eq!(EpochMode::parse("OFF")?, EpochMode::Off);
    assert_eq!(EpochMode::parse("")?, EpochMode::Off);
    assert_eq!("warn".parse::<EpochMode>()?, EpochMode::Warn);
    assert_eq!(EpochMode::default(), EpochMode::Off);
    let error = EpochMode::parse("sometimes").err().ok_or("sometimes must be refused")?;
    assert_eq!(
        error.to_string(),
        "SANDBOX_FENCE_EPOCH must be off, warn or require, got sometimes"
    );
    assert_eq!(
        "x".parse::<EpochMode>().err(),
        Some(EpochModeError { found: "x".to_owned() })
    );
    Ok(())
}

#[test]
fn a_generated_keypair_is_consistent_and_fresh() -> TestResult {
    let (seed, public) = DispatchSigner::generate_keypair_b64();
    let bytes = <[u8; 32]>::try_from(B64.decode(&seed)?).map_err(|_| "seed is not 32 bytes")?;
    assert_eq!(DispatchSigner::new("k1", bytes).public_key_b64(), public);
    assert_ne!(DispatchSigner::generate_keypair_b64().0, seed);
    Ok(())
}

#[test]
fn signing_replaces_an_envelope_already_in_the_body() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let once = sign(&signer, "t1", "l1")?;
    let twice = signer.sign_payload(&once, OP_LEASE_DISPATCH)?;
    let value: Value = serde_json::from_slice(&twice)?;
    assert!(value[AUTH_FIELD].is_object());
    check(&verifier, &twice, SystemTime::now())?;
    Ok(())
}

#[test]
fn an_epoch_in_the_target_is_signed() -> TestResult {
    let signer = DispatchSigner::new("k1", FORGE_SEED);
    let payload = serde_json::to_vec(&json!({
        "lease_id": "l1",
        "request": {"tenant": "t1"},
        "target": {"worker_id": "w1", "attempt_id": "a1", "epoch": 4},
    }))?;
    let signed = signer.sign_payload_at(&payload, OP_LEASE_DISPATCH, at(10))?;
    let value: Value = serde_json::from_slice(&signed)?;
    assert_eq!(value[AUTH_FIELD]["epoch"], 4);
    assert_eq!(value[AUTH_FIELD]["worker_id"], "w1");
    assert_eq!(value[AUTH_FIELD]["attempt_id"], "a1");
    Ok(())
}

#[test]
fn signing_refuses_a_payload_it_cannot_bind() -> TestResult {
    let signer = DispatchSigner::new("k1", FORGE_SEED);
    let sign = |value: Value| signer.sign_payload(&value.to_string().into_bytes(), OP_LEASE_DISPATCH);
    let not_json = signer.sign_payload(b"{", OP_LEASE_DISPATCH);
    assert!(matches!(&not_json, Err(DispatchSignError::NotJson(_))), "{not_json:?}");
    assert!(
        not_json
            .err()
            .ok_or("expected an error")?
            .to_string()
            .starts_with("dispatch is not JSON")
    );
    let result = sign(json!({"lease_id": "l1"}));
    assert!(matches!(result, Err(DispatchSignError::MissingTenant)), "{result:?}");
    let result = sign(json!({"lease_id": "l1", "request": {"tenant": 5}}));
    assert!(matches!(result, Err(DispatchSignError::MissingTenant)), "{result:?}");
    let result = sign(json!({"lease_id": "l1", "request": {"tenant": ""}}));
    assert!(matches!(result, Err(DispatchSignError::InvalidTenant(_))), "{result:?}");
    let result = sign(json!({"request": {"tenant": "t1"}}));
    assert!(matches!(result, Err(DispatchSignError::MissingLease)), "{result:?}");
    let result = sign(json!({"lease_id": "l1", "request": {"tenant": "t1"}, "target": {"worker_id": "bad worker"}}));
    assert!(matches!(result, Err(DispatchSignError::InvalidWorker(_))), "{result:?}");
    let result = sign(json!({"lease_id": "l1", "request": {"tenant": "t1"}, "target": {"attempt_id": ""}}));
    assert!(
        matches!(result, Err(DispatchSignError::InvalidAttempt(_))),
        "{result:?}"
    );
    let result = sign(json!({"lease_id": "l1", "request": {"tenant": "t1"}, "target": {"epoch": 0}}));
    assert!(matches!(result, Err(DispatchSignError::InvalidEpoch(_))), "{result:?}");
    Ok(())
}

#[test]
fn sign_errors_describe_what_is_wrong() -> TestResult {
    let signer = DispatchSigner::new("k1", FORGE_SEED);
    let describe = |value: Value| -> Result<String, Box<dyn std::error::Error>> {
        let error = signer
            .sign_payload(&value.to_string().into_bytes(), OP_LEASE_DISPATCH)
            .err()
            .ok_or("expected a refusal")?;
        Ok(error.to_string())
    };
    assert_eq!(describe(json!({"lease_id": "l1"}))?, "dispatch has no request tenant");
    assert_eq!(
        describe(json!({"request": {"tenant": "t1"}}))?,
        "dispatch has no lease id"
    );
    assert!(
        describe(json!({"lease_id": "l", "request": {"tenant": ""}}))?
            .starts_with("dispatch request tenant is invalid")
    );
    assert!(
        describe(json!({"lease_id": "l", "request": {"tenant": "t"}, "target": {"worker_id": "a b"}}))?
            .starts_with("dispatch target worker is invalid")
    );
    assert!(
        describe(json!({"lease_id": "l", "request": {"tenant": "t"}, "target": {"attempt_id": "a b"}}))?
            .starts_with("dispatch target attempt is invalid")
    );
    assert!(
        describe(json!({"lease_id": "l", "request": {"tenant": "t"}, "target": {"epoch": 0}}))?
            .starts_with("dispatch target epoch is invalid")
    );
    Ok(())
}

#[test]
fn public_keys_must_be_base64_of_a_valid_point() -> TestResult {
    for bad in [
        "not base64!!",
        "AAAA",
        "",
        &B64.encode([1u8; 31]),
        &B64.encode([1u8; 33]),
    ] {
        let result = DispatchVerifier::new(EnforcementMode::Require).with_key("k", bad);
        assert!(matches!(result, Err(DispatchPublicKeyError::Shape)), "{bad}");
    }
    let invalid = DispatchVerifier::new(EnforcementMode::Require)
        .with_key("k", &B64.encode(INVALID_POINT))
        .err()
        .ok_or("an off-curve key must be refused")?;
    assert!(matches!(invalid, DispatchPublicKeyError::Invalid(_)));
    assert!(invalid.to_string().starts_with("invalid dispatch public key"));
    let shape = DispatchVerifier::new(EnforcementMode::Require)
        .with_key("k", "AAAA")
        .err()
        .ok_or("a short key must be refused")?;
    assert_eq!(shape.to_string(), "dispatch public key must be base64 of 32 bytes");
    let (_, public) = DispatchSigner::generate_keypair_b64();
    DispatchVerifier::new(EnforcementMode::Require).with_key("k", &format!("  {public}\n"))?;
    Ok(())
}

const INVALID_POINT: [u8; 32] = [
    2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

#[test]
fn the_signer_reads_nothing_from_an_empty_environment() -> TestResult {
    assert!(signer_from(&vars(&[]))?.is_none());
    assert!(signer_from(&vars(&[(ENV_SIGNING_KEY, "")]))?.is_none());
    assert!(signer_from(&vars(&[(ENV_SIGNING_KEY_FILE, ""), (ENV_SIGNING_KEY, "")]))?.is_none());
    Ok(())
}

#[test]
fn the_signer_reads_its_seed_key_id_and_lifetime() -> TestResult {
    let (seed, public) = DispatchSigner::generate_keypair_b64();
    let signer = signer_from(&vars(&[(ENV_SIGNING_KEY, &seed)]))?.ok_or("a seed was set")?;
    assert_eq!(signer.key_id(), "k1");
    assert_eq!(signer.public_key_b64(), public);
    let signer = signer_from(&vars(&[
        (ENV_SIGNING_KEY, &format!(" {seed}\n")),
        (ENV_SIGNING_KEY_ID, "ops-2"),
        (ENV_SIGNING_TTL_SECS, "45"),
    ]))?
    .ok_or("a seed was set")?;
    assert_eq!(signer.key_id(), "ops-2");
    assert_eq!(signer.public_key_b64(), public);
    let signed = signer.sign_payload_at(&dispatch("t1", "l1")?, OP_LEASE_DISPATCH, at(100))?;
    let value: Value = serde_json::from_slice(&signed)?;
    assert_eq!(value[AUTH_FIELD]["expires_at"], 145);
    assert_eq!(value[AUTH_FIELD]["key_id"], "ops-2");
    let blank_id =
        signer_from(&vars(&[(ENV_SIGNING_KEY, &seed), (ENV_SIGNING_KEY_ID, "")]))?.ok_or("a seed was set")?;
    assert_eq!(blank_id.key_id(), "k1");
    Ok(())
}

#[test]
fn the_signer_prefers_a_key_file_over_the_inline_seed() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("signing.key");
    let (file_seed, file_public) = DispatchSigner::generate_keypair_b64();
    let (inline_seed, _) = DispatchSigner::generate_keypair_b64();
    std::fs::write(&path, format!("{file_seed}\n"))?;
    let path_text = path.to_string_lossy().into_owned();
    let signer = signer_from(&vars(&[
        (ENV_SIGNING_KEY_FILE, &path_text),
        (ENV_SIGNING_KEY, &inline_seed),
    ]))?
    .ok_or("a key file was set")?;
    assert_eq!(signer.public_key_b64(), file_public);
    let fallback = signer_from(&vars(&[(ENV_SIGNING_KEY_FILE, ""), (ENV_SIGNING_KEY, &inline_seed)]))?
        .ok_or("an inline seed was set")?;
    assert_ne!(fallback.public_key_b64(), file_public);
    Ok(())
}

#[test]
fn the_signer_reports_an_unreadable_key_file() -> TestResult {
    let directory = tempfile::tempdir()?;
    let missing = directory.path().join("absent.key");
    let path_text = missing.to_string_lossy().into_owned();
    let error = signer_from(&vars(&[(ENV_SIGNING_KEY_FILE, &path_text)]))
        .err()
        .ok_or("a missing file must be refused")?;
    assert!(
        matches!(&error, DispatchSignerConfigError::ReadKeyFile { path, .. } if *path == path_text),
        "{error:?}"
    );
    assert!(error.to_string().contains(ENV_SIGNING_KEY_FILE));
    assert!(std::error::Error::source(&error).is_some());
    Ok(())
}

#[test]
fn the_signer_refuses_a_malformed_seed_key_id_or_lifetime() -> TestResult {
    let (seed, _) = DispatchSigner::generate_keypair_b64();
    for bad in ["not base64 !!", &B64.encode([1u8; 31]), &B64.encode([1u8; 33]), "   x"] {
        let error = signer_from(&vars(&[(ENV_SIGNING_KEY, bad)]))
            .err()
            .ok_or("a bad seed must be refused")?;
        assert!(matches!(error, DispatchSignerConfigError::InvalidSeed), "{bad}");
    }
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("short.key");
    std::fs::write(&path, "AAAA")?;
    let path_text = path.to_string_lossy().into_owned();
    let error = signer_from(&vars(&[(ENV_SIGNING_KEY_FILE, &path_text)]))
        .err()
        .ok_or("a short key file must be refused")?;
    assert!(matches!(error, DispatchSignerConfigError::InvalidSeed));
    assert_eq!(error.to_string(), "dispatch signing key must be a base64 32-byte seed");
    for id in ["a:b", "a,b", "a\nb"] {
        let error = signer_from(&vars(&[(ENV_SIGNING_KEY, &seed), (ENV_SIGNING_KEY_ID, id)]))
            .err()
            .ok_or("a bad key id must be refused")?;
        assert!(matches!(error, DispatchSignerConfigError::InvalidKeyId), "{id:?}");
        assert!(error.to_string().contains(ENV_SIGNING_KEY_ID));
    }
    for ttl in ["0", "-3", "soon", "", "1.5"] {
        let error = signer_from(&vars(&[(ENV_SIGNING_KEY, &seed), (ENV_SIGNING_TTL_SECS, ttl)]))
            .err()
            .ok_or("a bad lifetime must be refused")?;
        assert!(matches!(error, DispatchSignerConfigError::InvalidTtl), "{ttl:?}");
        assert!(error.to_string().contains(ENV_SIGNING_TTL_SECS));
    }
    Ok(())
}

#[test]
fn the_process_environment_readers_accept_an_unconfigured_environment() {
    // Neither reader may panic whatever the host sets; with the variables unset they succeed.
    let _ = DispatchSigner::from_env();
    let _ = DispatchVerifier::from_env();
}

#[test]
fn the_verifier_defaults_to_warn_without_keys() -> TestResult {
    let verifier = verifier_from(&vars(&[]), None)?;
    assert_eq!(verifier.mode(), EnforcementMode::Warn);
    let decoded: Value = verifier.decode(&dispatch("t1", "l1")?, OP_LEASE_DISPATCH, None)?;
    assert_eq!(decoded["lease_id"], "l1");
    Ok(())
}

#[test]
fn the_verifier_reads_its_mode_and_inline_keys() -> TestResult {
    let signer = DispatchSigner::new("k1", random_seed());
    let public = signer.public_key_b64();
    let (_, second) = DispatchSigner::generate_keypair_b64();
    let listing = format!("# trusted keys\nk1: {public}\n\n  k2 :{second} , \n#k3:ignored");
    let configured = verifier_from(
        &vars(&[(ENV_VERIFY_MODE, "Require"), (ENV_PUBLIC_KEYS, &listing)]),
        None,
    )?;
    assert_eq!(configured.mode(), EnforcementMode::Require);
    check(&configured, &sign(&signer, "t1", "l1")?, SystemTime::now())?;
    let off = verifier_from(&vars(&[(ENV_VERIFY_MODE, "off")]), None)?;
    assert_eq!(off.mode(), EnforcementMode::Off);
    let comma = verifier_from(&vars(&[(ENV_PUBLIC_KEYS, &format!("k1:{public},k2:{second}"))]), None)?;
    check(&comma, &sign(&signer, "t1", "l1")?, SystemTime::now())?;
    Ok(())
}

#[test]
fn the_verifier_reads_keys_from_a_file_in_preference_to_the_inline_list() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("keys");
    let signer = DispatchSigner::new("k1", random_seed());
    std::fs::write(&path, format!("k1:{}\n", signer.public_key_b64()))?;
    let (_, other) = DispatchSigner::generate_keypair_b64();
    let path_text = path.to_string_lossy().into_owned();
    let verifier = verifier_from(
        &vars(&[
            (ENV_VERIFY_MODE, "require"),
            (ENV_PUBLIC_KEYS_FILE, &path_text),
            (ENV_PUBLIC_KEYS, &format!("k1:{other}")),
        ]),
        None,
    )?;
    check(&verifier, &sign(&signer, "t1", "l1")?, SystemTime::now())?;
    let blank_path = verifier_from(
        &vars(&[
            (ENV_VERIFY_MODE, "require"),
            (ENV_PUBLIC_KEYS_FILE, ""),
            (ENV_PUBLIC_KEYS, &format!("k1:{}", signer.public_key_b64())),
        ]),
        None,
    )?;
    check(&blank_path, &sign(&signer, "t1", "l2")?, SystemTime::now())?;
    Ok(())
}

#[test]
fn the_verifier_refuses_bad_configuration() -> TestResult {
    let error = verifier_from(&vars(&[(ENV_VERIFY_MODE, "maybe")]), None)
        .err()
        .ok_or("a bad mode must be refused")?;
    assert!(matches!(error, DispatchVerifierConfigError::Mode(_)));
    assert!(error.to_string().contains("got maybe"));
    let error = verifier_from(&vars(&[(ENV_FENCE_EPOCH, "sometimes")]), None)
        .err()
        .ok_or("a bad epoch mode must be refused")?;
    assert!(matches!(error, DispatchVerifierConfigError::Epoch(_)));
    let error = verifier_from(&vars(&[(ENV_PUBLIC_KEYS, "no-separator")]), None)
        .err()
        .ok_or("a malformed entry must be refused")?;
    assert!(matches!(error, DispatchVerifierConfigError::KeyEntryMalformed));
    assert_eq!(error.to_string(), "dispatch public key entries look like key_id:base64");
    let error = verifier_from(&vars(&[(ENV_PUBLIC_KEYS, "k1:AAAA")]), None)
        .err()
        .ok_or("a short key must be refused")?;
    assert!(matches!(
        error,
        DispatchVerifierConfigError::Key(DispatchPublicKeyError::Shape)
    ));
    let error = verifier_from(&vars(&[(ENV_VERIFY_MODE, "require")]), None)
        .err()
        .ok_or("require without keys must be refused")?;
    assert!(matches!(error, DispatchVerifierConfigError::RequireWithoutKeys));
    assert!(error.to_string().contains(ENV_VERIFY_MODE));
    let error = verifier_from(
        &vars(&[(ENV_VERIFY_MODE, "require"), (ENV_PUBLIC_KEYS, "# only a comment")]),
        None,
    )
    .err()
    .ok_or("comments are not keys")?;
    assert!(matches!(error, DispatchVerifierConfigError::RequireWithoutKeys));
    Ok(())
}

#[test]
fn the_verifier_reports_an_unreadable_key_file() -> TestResult {
    let directory = tempfile::tempdir()?;
    let missing = directory.path().join("absent");
    let path_text = missing.to_string_lossy().into_owned();
    let error = verifier_from(&vars(&[(ENV_PUBLIC_KEYS_FILE, &path_text)]), None)
        .err()
        .ok_or("a missing file must be refused")?;
    assert!(
        matches!(&error, DispatchVerifierConfigError::KeysFile { path, .. } if *path == path_text),
        "{error:?}"
    );
    assert!(error.to_string().contains(ENV_PUBLIC_KEYS_FILE));
    assert!(std::error::Error::source(&error).is_some());
    Ok(())
}

#[test]
fn the_epoch_mode_variable_defaults_to_warn_and_is_honoured() -> TestResult {
    let signer = DispatchSigner::new("k1", random_seed());
    let public = signer.public_key_b64();
    let build = |epoch: Option<&str>| -> Result<DispatchVerifier, Box<dyn std::error::Error>> {
        let mut pairs = vec![(ENV_VERIFY_MODE, "require")];
        let keys = format!("k1:{public}");
        pairs.push((ENV_PUBLIC_KEYS, &keys));
        if let Some(epoch) = epoch {
            pairs.push((ENV_FENCE_EPOCH, epoch));
        }
        Ok(verifier_from(&vars(&pairs), None)?)
    };
    let dispatch_at_epoch = |epoch: u64| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let payload = serde_json::to_vec(&json!({
            "lease_id": "l1",
            "request": {"tenant": "t1"},
            "target": {"worker_id": "w1", "attempt_id": "a1", "epoch": epoch},
        }))?;
        Ok(signer.sign_payload(&payload, OP_LEASE_DISPATCH)?)
    };
    for lenient in [None, Some("off"), Some("warn")] {
        let verifier = build(lenient)?;
        check(&verifier, &dispatch_at_epoch(5)?, SystemTime::now())?;
        check(&verifier, &dispatch_at_epoch(2)?, SystemTime::now())?;
    }
    let strict = build(Some("require"))?;
    check(&strict, &dispatch_at_epoch(5)?, SystemTime::now())?;
    let result = check(&strict, &dispatch_at_epoch(2)?, SystemTime::now());
    assert!(matches!(result, Err(DispatchAuthError::StaleEpoch)), "{result:?}");
    Ok(())
}

#[test]
fn the_verifier_keeps_its_fence_in_the_configured_journal() -> TestResult {
    let directory = tempfile::tempdir()?;
    let configured = directory.path().join("configured.jsonl");
    let default = directory.path().join("default.jsonl");
    let signer = DispatchSigner::new("k1", random_seed());
    let keys = format!("k1:{}", signer.public_key_b64());
    let configured_text = configured.to_string_lossy().into_owned();
    let env = vars(&[
        (ENV_VERIFY_MODE, "require"),
        (ENV_PUBLIC_KEYS, &keys),
        (ENV_FENCE_FILE, &configured_text),
    ]);
    let verifier = verifier_from(&env, Some(default.clone()))?;
    check(&verifier, &sign(&signer, "t1", "l1")?, SystemTime::now())?;
    assert!(configured.exists());
    assert!(!default.exists());
    let blank = vars(&[
        (ENV_VERIFY_MODE, "require"),
        (ENV_PUBLIC_KEYS, &keys),
        (ENV_FENCE_FILE, ""),
    ]);
    let verifier = verifier_from(&blank, Some(default.clone()))?;
    check(&verifier, &sign(&signer, "t1", "l1")?, SystemTime::now())?;
    assert!(default.exists());
    Ok(())
}

#[test]
fn an_unusable_fence_journal_leaves_the_verifier_working_in_memory() -> TestResult {
    let directory = tempfile::tempdir()?;
    let blocker = directory.path().join("blocker");
    std::fs::write(&blocker, "file")?;
    let journal = blocker.join("fence.jsonl");
    let signer = DispatchSigner::new("k1", random_seed());
    let keys = format!("k1:{}", signer.public_key_b64());
    let verifier = verifier_from(
        &vars(&[(ENV_VERIFY_MODE, "require"), (ENV_PUBLIC_KEYS, &keys)]),
        Some(journal.clone()),
    )?;
    check(&verifier, &sign(&signer, "t1", "l1")?, SystemTime::now())?;
    let result = verifier.fresh_command_launch_fence();
    assert!(
        matches!(result, Err(FreshLaunchFenceError::EpochRequired)),
        "{result:?}"
    );
    assert!(
        DispatchVerifier::new(EnforcementMode::Require)
            .with_fence_file(&journal)
            .is_err()
    );
    Ok(())
}

#[test]
fn a_dispatch_for_an_empty_lease_or_attempt_is_still_fenced() -> TestResult {
    let (signer, verifier) = pair(EnforcementMode::Require)?;
    let payload = |tenant: &str| serde_json::to_vec(&json!({"lease_id": "", "request": {"tenant": tenant}}));
    check(
        &verifier,
        &signer.sign_payload(&payload("t1")?, OP_LEASE_DISPATCH)?,
        SystemTime::now(),
    )?;
    let result = check(
        &verifier,
        &signer.sign_payload(&payload("t2")?, OP_LEASE_DISPATCH)?,
        SystemTime::now(),
    );
    assert!(
        matches!(result, Err(DispatchAuthError::LeaseTenantConflict)),
        "{result:?}"
    );
    Ok(())
}

#[test]
fn auth_errors_describe_the_refusal() {
    let cases = [
        (DispatchAuthError::Missing, "dispatch is not signed"),
        (DispatchAuthError::BadSignature, "dispatch signature does not verify"),
        (DispatchAuthError::Expired, "dispatch signature has expired"),
        (
            DispatchAuthError::BodyMismatch,
            "dispatch body differs from what was signed",
        ),
        (
            DispatchAuthError::TenantMismatch,
            "dispatch tenant differs from the signed tenant",
        ),
        (
            DispatchAuthError::LeaseTenantConflict,
            "lease id is already held for a different tenant",
        ),
        (
            DispatchAuthError::LeaseMismatch,
            "signed lease differs from the request's lease",
        ),
        (
            DispatchAuthError::StaleAttempt,
            "request is for an attempt that is not the live one for this lease",
        ),
        (
            DispatchAuthError::StaleEpoch,
            "request carries an older lease epoch than the one held for this lease",
        ),
        (DispatchAuthError::Replayed, "signed request was already seen"),
        (
            DispatchAuthError::UnknownKey("k9".to_owned()),
            "dispatch signed with unknown key k9",
        ),
        (
            DispatchAuthError::WrongOperation {
                expected: "a".to_owned(),
                found: "b".to_owned(),
            },
            "dispatch signed for operation b, expected a",
        ),
        (
            DispatchAuthError::WrongWorker {
                expected: "w1".to_owned(),
                found: "w2".to_owned(),
            },
            "dispatch signed for worker w2, this worker is w1",
        ),
        (
            DispatchAuthError::Malformed(DispatchMalformedReason::Nonce),
            "dispatch signature is malformed: nonce",
        ),
    ];
    for (error, text) in cases {
        assert_eq!(error.to_string(), text);
    }
}

#[test]
fn malformed_reasons_describe_what_was_unreadable() {
    let cases = [
        (DispatchMalformedReason::NotAnObject, "not an object"),
        (DispatchMalformedReason::UnsupportedVersion { version: 3 }, "version 3"),
        (DispatchMalformedReason::Signature, "signature"),
        (DispatchMalformedReason::Nonce, "nonce"),
        (
            DispatchMalformedReason::ReadinessTransfer,
            "command readiness cannot carry a transfer descriptor",
        ),
        (
            DispatchMalformedReason::NativeSnapshotDeleteTransfer,
            "native snapshot deletion cannot carry a transfer descriptor",
        ),
        (
            DispatchMalformedReason::OwnedCancellationTransfer,
            "owned cancellation cannot carry a transfer descriptor",
        ),
        (
            DispatchMalformedReason::NativeSnapshotProvider,
            "native snapshot provider differs from its subject",
        ),
        (
            DispatchMalformedReason::CapturedScopeNotReadiness,
            "captured scope is not worker command readiness",
        ),
    ];
    for (reason, text) in cases {
        assert_eq!(reason.to_string(), text);
    }
}

#[test]
fn fresh_launch_fence_errors_describe_what_is_missing() {
    let cases = [
        (
            FreshLaunchFenceError::VerificationRequired,
            "fresh command launch requires signature enforcement",
        ),
        (
            FreshLaunchFenceError::VerificationKeysUnavailable,
            "fresh command launch requires verification keys",
        ),
        (
            FreshLaunchFenceError::EpochRequired,
            "fresh command launch requires epoch enforcement",
        ),
        (
            FreshLaunchFenceError::JournalUnavailable,
            "fresh command launch requires a persistent dispatch fence",
        ),
        (
            FreshLaunchFenceError::UntrustedJournal,
            "fresh command launch requires a trusted dispatch fence",
        ),
        (
            FreshLaunchFenceError::UnsupportedPlatform,
            "fresh command launch fence identity is unearned on this platform",
        ),
        (
            FreshLaunchFenceError::JournalIoFailed,
            "fresh command launch dispatch fence durability failed",
        ),
        (
            FreshLaunchFenceError::JournalIdentityUnavailable,
            "fresh command launch dispatch fence identity is unavailable",
        ),
        (
            FreshLaunchFenceError::JournalChanged,
            "fresh command launch dispatch fence identity or length changed",
        ),
    ];
    for (error, text) in cases {
        assert_eq!(error.to_string(), text);
    }
}

#[test]
fn decode_errors_pass_their_cause_through() -> TestResult {
    let json_error = serde_json::from_str::<Value>("{")
        .err()
        .ok_or("expected a parse error")?;
    let text = json_error.to_string();
    assert_eq!(DispatchDecodeError::from(json_error).to_string(), text);
    assert_eq!(
        DispatchDecodeError::from(DispatchAuthError::Missing).to_string(),
        "dispatch is not signed"
    );
    Ok(())
}
