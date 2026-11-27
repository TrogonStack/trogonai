//! Lease epochs: how they are signed, how each epoch mode treats a lower one, and how the fence keeps them.

use std::path::Path;
use std::time::SystemTime;

use serde_json::{Value, json};
use trogon_sandbox_contract::{DispatchAttemptId, DispatchAuth, FileSha256, LeaseEpoch, LeaseId, TenantId, WorkerId};
use trogon_sandbox_protocol::{
    AUTH_FIELD, DispatchAuthError, DispatchSigner, DispatchVerifier, EnforcementMode, EpochMode, FreshLaunchFenceError,
    OP_LEASE_DISPATCH, VerbScope,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const RUN: &str = "sandbox.provider.firecracker.run_command.l1";
const CLEANUP: &str = "sandbox.provider.firecracker.worker.w1.cleanup";

fn random_seed() -> [u8; 32] {
    let mut seed = [0u8; 32];
    seed[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    seed[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    seed
}

fn dispatch_at(worker: Option<&str>, attempt: &str, epoch: Option<u64>) -> Result<Vec<u8>, serde_json::Error> {
    let mut target = json!({"attempt_id": attempt});
    if let Some(worker) = worker {
        target["worker_id"] = worker.into();
    }
    if let Some(epoch) = epoch {
        target["epoch"] = epoch.into();
    }
    serde_json::to_vec(&json!({
        "lease_id": "l1",
        "request": {"tenant": "t1"},
        "target": target,
    }))
}

fn scope(attempt: &str, epoch: Option<u64>) -> Result<VerbScope, Box<dyn std::error::Error>> {
    Ok(VerbScope {
        tenant: TenantId::new("t1")?,
        lease_id: LeaseId::new("l1"),
        worker_id: Some(WorkerId::new("w1")?),
        attempt_id: Some(DispatchAttemptId::new(attempt)?),
        epoch: epoch.map(LeaseEpoch::new).transpose()?,
    })
}

fn signed(signer: &DispatchSigner, attempt: &str, epoch: Option<u64>) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(signer.sign_payload(&dispatch_at(Some("w1"), attempt, epoch)?, OP_LEASE_DISPATCH)?)
}

fn verifier(
    enforcement: EnforcementMode,
    epoch: EpochMode,
) -> Result<(DispatchSigner, DispatchVerifier), Box<dyn std::error::Error>> {
    let signer = DispatchSigner::new("k1", random_seed());
    let verifier = DispatchVerifier::new(enforcement)
        .with_epoch_mode(epoch)
        .with_key("k1", &signer.public_key_b64())?;
    Ok((signer, verifier))
}

fn admit(
    verifier: &DispatchVerifier,
    signer: &DispatchSigner,
    attempt: &str,
    epoch: Option<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut value: Value = serde_json::from_slice(&signed(signer, attempt, epoch)?)?;
    Ok(verifier.verify_value(&mut value, OP_LEASE_DISPATCH, None, SystemTime::now())?)
}

fn refused_as_stale_epoch(result: Result<(), Box<dyn std::error::Error>>) -> bool {
    matches!(
        result
            .err()
            .and_then(|error| error.downcast::<DispatchAuthError>().ok())
            .as_deref(),
        Some(DispatchAuthError::StaleEpoch)
    )
}

fn journaled(
    path: &Path,
    signer: &DispatchSigner,
    epoch: EpochMode,
) -> Result<DispatchVerifier, Box<dyn std::error::Error>> {
    Ok(DispatchVerifier::new(EnforcementMode::Require)
        .with_epoch_mode(epoch)
        .with_key("k1", &signer.public_key_b64())?
        .with_fence_file(path)?)
}

fn auth_with(epoch: Option<LeaseEpoch>) -> Result<DispatchAuth, Box<dyn std::error::Error>> {
    Ok(DispatchAuth {
        version: 1,
        key_id: "k1".to_owned(),
        operation: "op".to_owned(),
        tenant: TenantId::new("t1")?,
        lease_id: LeaseId::new("l1"),
        worker_id: Some(WorkerId::new("w1")?),
        attempt_id: Some(DispatchAttemptId::new("a1")?),
        nonce: Some("n1".to_owned()),
        epoch,
        expires_at: 1_000,
        body_sha256: FileSha256::new("ab".repeat(32))?,
        signature: String::new(),
    })
}

#[test]
fn an_epochless_envelope_serializes_without_the_field_and_old_json_parses() -> TestResult {
    let header = serde_json::to_value(auth_with(None)?)?;
    assert!(header.get("epoch").is_none());
    let parsed: DispatchAuth = serde_json::from_value(header)?;
    assert_eq!(parsed.epoch, None);
    let stamped = serde_json::to_value(auth_with(Some(LeaseEpoch::new(3)?))?)?;
    assert_eq!(stamped["epoch"], 3);
    let zero = serde_json::from_value::<DispatchAuth>({
        let mut value = serde_json::to_value(auth_with(None)?)?;
        value["epoch"] = 0.into();
        value
    });
    assert!(zero.is_err(), "an epoch of zero is never accepted");
    Ok(())
}

#[test]
fn the_epoch_in_a_signed_dispatch_comes_from_the_target_and_is_covered_by_the_signature() -> TestResult {
    let (signer, verifier) = verifier(EnforcementMode::Require, EpochMode::Off)?;
    let bytes = signed(&signer, "a1", Some(4))?;
    let mut value: Value = serde_json::from_slice(&bytes)?;
    assert_eq!(value[AUTH_FIELD]["epoch"], 4);
    verifier.verify_value(&mut value, OP_LEASE_DISPATCH, None, SystemTime::now())?;

    let mut forged: Value = serde_json::from_slice(&signed(&signer, "a1", Some(4))?)?;
    forged[AUTH_FIELD]["epoch"] = 9.into();
    let result = verifier.verify_value(&mut forged, OP_LEASE_DISPATCH, None, SystemTime::now());
    assert!(matches!(result, Err(DispatchAuthError::BadSignature)), "{result:?}");

    let plain: Value = serde_json::from_slice(&signed(&signer, "a1", None)?)?;
    assert!(plain[AUTH_FIELD].get("epoch").is_none());
    Ok(())
}

#[test]
fn a_signed_epoch_changes_the_signature() -> TestResult {
    let signer = DispatchSigner::new("k1", [7u8; 32]);
    let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
    let signature = |epoch: Option<u64>| -> Result<Value, Box<dyn std::error::Error>> {
        let bytes = signer.sign_payload_at(&dispatch_at(Some("w1"), "a1", epoch)?, OP_LEASE_DISPATCH, now)?;
        let value: Value = serde_json::from_slice(&bytes)?;
        Ok(value[AUTH_FIELD]["signature"].clone())
    };
    assert_eq!(signature(Some(7))?, signature(Some(7))?);
    assert_ne!(signature(Some(7))?, signature(None)?);
    assert_ne!(signature(Some(7))?, signature(Some(8))?);
    Ok(())
}

#[test]
fn off_tracks_but_never_refuses_a_lower_epoch() -> TestResult {
    let (signer, verifier) = verifier(EnforcementMode::Require, EpochMode::Off)?;
    admit(&verifier, &signer, "a1", Some(5))?;
    admit(&verifier, &signer, "a1", Some(2))?;
    admit(&verifier, &signer, "a1", None)?;
    Ok(())
}

#[test]
fn warn_accepts_a_lower_epoch() -> TestResult {
    let (signer, verifier) = verifier(EnforcementMode::Require, EpochMode::Warn)?;
    admit(&verifier, &signer, "a1", Some(5))?;
    admit(&verifier, &signer, "a1", Some(2))?;
    Ok(())
}

#[test]
fn require_refuses_a_lower_epoch_and_accepts_equal_higher_and_absent() -> TestResult {
    let (signer, verifier) = verifier(EnforcementMode::Require, EpochMode::Require)?;
    admit(&verifier, &signer, "a1", Some(5))?;
    assert!(refused_as_stale_epoch(admit(&verifier, &signer, "a1", Some(4))));
    admit(&verifier, &signer, "a1", Some(5))?;
    admit(&verifier, &signer, "a1", Some(6))?;
    assert!(
        refused_as_stale_epoch(admit(&verifier, &signer, "a1", Some(5))),
        "the higher epoch was adopted"
    );
    admit(&verifier, &signer, "a1", None)?;
    Ok(())
}

#[test]
fn require_is_not_effective_unless_signing_is_required() -> TestResult {
    let (signer, verifier) = verifier(EnforcementMode::Warn, EpochMode::Require)?;
    admit(&verifier, &signer, "a1", Some(5))?;
    admit(&verifier, &signer, "a1", Some(1))?;
    assert!(matches!(
        verifier.fresh_command_launch_fence(),
        Err(FreshLaunchFenceError::VerificationRequired)
    ));
    Ok(())
}

#[test]
fn a_verb_and_a_cleanup_with_a_lower_epoch_are_refused_under_require() -> TestResult {
    let (signer, verifier) = verifier(EnforcementMode::Require, EpochMode::Require)?;
    admit(&verifier, &signer, "a1", Some(3))?;
    let body = b"{}";
    let lower = signer.sign_verb(RUN, body, &scope("a1", Some(2))?)?;
    let result = verifier.verify_verb(RUN, &lower, body, false, SystemTime::now());
    assert!(matches!(result, Err(DispatchAuthError::StaleEpoch)), "{result:?}");
    let equal = signer.sign_verb(RUN, body, &scope("a1", Some(3))?)?;
    verifier.verify_verb(RUN, &equal, body, false, SystemTime::now())?;
    let higher = signer.sign_verb(RUN, body, &scope("a1", Some(4))?)?;
    verifier.verify_verb(RUN, &higher, body, false, SystemTime::now())?;
    let cleanup = serde_json::to_vec(&json!({
        "identity": {"lease_id": "l1", "provider": "firecracker",
            "target": {"worker_id": "w1", "attempt_id": "a1", "epoch": 2}},
        "intent": "release",
    }))?;
    let header = signer.sign_verb(CLEANUP, &cleanup, &scope("a1", Some(2))?)?;
    let result = verifier.verify_verb(CLEANUP, &header, &cleanup, false, SystemTime::now());
    assert!(matches!(result, Err(DispatchAuthError::StaleEpoch)), "{result:?}");
    Ok(())
}

#[test]
fn a_verb_epoch_is_not_checked_under_off_or_warn() -> TestResult {
    for mode in [EpochMode::Off, EpochMode::Warn] {
        let (signer, verifier) = verifier(EnforcementMode::Require, mode)?;
        admit(&verifier, &signer, "a1", Some(3))?;
        let body = b"{}";
        let lower = signer.sign_verb(RUN, body, &scope("a1", Some(2))?)?;
        verifier.verify_verb(RUN, &lower, body, false, SystemTime::now())?;
    }
    Ok(())
}

#[test]
fn the_epoch_survives_a_restart() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    let signer = DispatchSigner::new("k1", random_seed());
    admit(&journaled(&path, &signer, EpochMode::Require)?, &signer, "a1", Some(5))?;
    let restarted = journaled(&path, &signer, EpochMode::Require)?;
    assert!(refused_as_stale_epoch(admit(&restarted, &signer, "a1", Some(4))));
    admit(&restarted, &signer, "a1", Some(5))?;
    Ok(())
}

#[test]
fn a_lease_journaled_before_epochs_holds_epoch_zero() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("old.jsonl");
    std::fs::write(&path, "{\"l\":\"l1\",\"t\":\"t1\",\"a\":\"a1\",\"c\":false}\n")?;
    let signer = DispatchSigner::new("k1", random_seed());
    let verifier = journaled(&path, &signer, EpochMode::Require)?;
    admit(&verifier, &signer, "a1", Some(1))?;
    admit(&verifier, &signer, "a1", None)?;
    Ok(())
}

#[test]
fn an_epochless_journal_line_stays_byte_identical() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    let signer = DispatchSigner::new("k1", random_seed());
    let verifier = journaled(&path, &signer, EpochMode::Off)?;
    let payload = signer.sign_payload(&dispatch_at(None, "a1", None)?, OP_LEASE_DISPATCH)?;
    let mut value: Value = serde_json::from_slice(&payload)?;
    verifier.verify_value(&mut value, OP_LEASE_DISPATCH, None, SystemTime::now())?;
    assert_eq!(
        std::fs::read_to_string(&path)?,
        "{\"l\":\"l1\",\"t\":\"t1\",\"a\":\"a1\",\"c\":false,\"x\":false}\n"
    );
    Ok(())
}

#[test]
fn the_held_epoch_and_worker_are_journaled_and_raised_in_place() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    let signer = DispatchSigner::new("k1", random_seed());
    let verifier = journaled(&path, &signer, EpochMode::Require)?;
    admit(&verifier, &signer, "a1", Some(5))?;
    assert_eq!(
        std::fs::read_to_string(&path)?,
        "{\"l\":\"l1\",\"t\":\"t1\",\"w\":\"w1\",\"a\":\"a1\",\"c\":false,\"e\":5,\"x\":false}\n"
    );
    admit(&verifier, &signer, "a1", Some(5))?;
    assert_eq!(
        std::fs::read_to_string(&path)?.lines().count(),
        1,
        "a repeat writes nothing"
    );
    admit(&verifier, &signer, "a1", Some(6))?;
    let text = std::fs::read_to_string(&path)?;
    assert_eq!(text.lines().count(), 2);
    assert!(text.ends_with("\"e\":6,\"x\":false}\n"), "{text}");
    Ok(())
}

#[test]
fn a_cleanup_raises_the_epoch_and_a_repeat_changes_nothing() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    let signer = DispatchSigner::new("k1", random_seed());
    let verifier = journaled(&path, &signer, EpochMode::Require)?;
    admit(&verifier, &signer, "a1", Some(5))?;
    let cleanup = serde_json::to_vec(&json!({
        "identity": {"lease_id": "l1", "provider": "firecracker",
            "target": {"worker_id": "w1", "attempt_id": "a1", "epoch": 6}},
        "intent": "release",
    }))?;
    let header = signer.sign_verb(CLEANUP, &cleanup, &scope("a1", Some(6))?)?;
    verifier.verify_verb(CLEANUP, &header, &cleanup, false, SystemTime::now())?;
    let text = std::fs::read_to_string(&path)?;
    assert!(
        text.ends_with("{\"l\":\"l1\",\"t\":\"t1\",\"w\":\"w1\",\"a\":\"a1\",\"c\":true,\"e\":6,\"x\":false}\n"),
        "{text}"
    );
    let lines = text.lines().count();
    let again = signer.sign_verb(CLEANUP, &cleanup, &scope("a1", Some(6))?)?;
    verifier.verify_verb(CLEANUP, &again, &cleanup, false, SystemTime::now())?;
    assert_eq!(std::fs::read_to_string(&path)?.lines().count(), lines);
    let lower = signer.sign_verb(CLEANUP, &cleanup, &scope("a1", Some(2))?)?;
    let result = verifier.verify_verb(CLEANUP, &lower, &cleanup, false, SystemTime::now());
    assert!(matches!(result, Err(DispatchAuthError::StaleEpoch)), "{result:?}");
    Ok(())
}
