//! Which verified dispatches a restarted worker may recover a tenant from.

use std::error::Error;
use std::io::Write as _;
use std::path::Path;
use std::time::SystemTime;

use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CancelIntent, DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderCleanupRequest,
    ProviderKind, TenantId, WorkerId,
};
use trogon_sandbox_protocol::{
    DispatchAuthError, DispatchDecodeError, DispatchSigner, DispatchVerifier, EnforcementMode, EpochMode,
    OP_LEASE_DISPATCH, VerbScope,
};

const RUN_COMMAND_SUBJECT: &str = "sandbox.provider.e2b.run_command.recovered-lease";

fn target(epoch: u64) -> Result<DispatchTarget, Box<dyn Error>> {
    Ok(DispatchTarget {
        worker_id: WorkerId::new("recovered-worker")?,
        attempt_id: DispatchAttemptId::new("recovered-attempt")?,
        epoch: Some(LeaseEpoch::new(epoch)?),
    })
}

fn lease() -> LeaseId {
    LeaseId::new("recovered-lease")
}

fn original_tenant() -> Result<TenantId, Box<dyn Error>> {
    Ok(TenantId::new("original-tenant")?)
}

fn signer() -> DispatchSigner {
    DispatchSigner::new("recovered-fixture", [67; 32])
}

fn verifier(signer: &DispatchSigner, mode: EnforcementMode, path: &Path) -> Result<DispatchVerifier, Box<dyn Error>> {
    Ok(DispatchVerifier::new(mode)
        .with_epoch_mode(EpochMode::Require)
        .with_key(signer.key_id(), &signer.public_key_b64())?
        .with_fence_file(path)?)
}

fn dispatch(
    verifier: &DispatchVerifier,
    signer: &DispatchSigner,
    target: &DispatchTarget,
) -> Result<(), Box<dyn Error>> {
    let payload = serde_json::to_vec(&json!({
        "lease_id": lease(),
        "target": target,
        "request": {"tenant": "original-tenant", "env": {"REQUEST_ONLY": "fixture-request-body-value"}},
    }))?;
    let signed = signer.sign_payload(&payload, OP_LEASE_DISPATCH)?;
    verifier.decode::<Value>(&signed, OP_LEASE_DISPATCH, Some(&target.worker_id))?;
    Ok(())
}

fn seed(path: &Path, signer: &DispatchSigner) -> Result<(), Box<dyn Error>> {
    let producer = verifier(signer, EnforcementMode::Require, path)?;
    dispatch(&producer, signer, &target(7)?)?;
    assert_eq!(producer.recovered_dispatch_tenant(&lease(), &target(7)?), None);
    Ok(())
}

fn cleanup(
    verifier: &DispatchVerifier,
    signer: &DispatchSigner,
    target: &DispatchTarget,
    epoch: u64,
) -> Result<(), Box<dyn Error>> {
    let request = ProviderCleanupRequest {
        identity: LeaseAttemptIdentity {
            lease_id: lease(),
            provider: ProviderKind::E2b,
            target: target.clone(),
        },
        intent: CancelIntent::Release,
    };
    let subject = format!("sandbox.provider.e2b.worker.{}.cleanup", target.worker_id);
    let body = serde_json::to_vec(&request)?;
    let scope = VerbScope {
        tenant: original_tenant()?,
        lease_id: lease(),
        worker_id: Some(target.worker_id.clone()),
        attempt_id: Some(target.attempt_id.clone()),
        epoch: Some(LeaseEpoch::new(epoch)?),
    };
    let header = signer.sign_verb(&subject, &body, &scope)?;
    verifier.verify_verb(&subject, &header, &body, false, SystemTime::now())?;
    Ok(())
}

#[test]
fn only_an_exact_file_recovered_verified_dispatch_supplies_its_tenant() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    let signer = signer();
    seed(&path, &signer)?;
    let retained = verifier(&signer, EnforcementMode::Require, &path)?;
    assert_eq!(
        retained.recovered_dispatch_tenant(&lease(), &target(7)?),
        Some(original_tenant()?)
    );
    assert_eq!(
        retained.recovered_dispatch_tenant(&LeaseId::new("another-lease"), &target(7)?),
        None
    );
    let mut foreign = target(7)?;
    foreign.worker_id = WorkerId::new("another-worker")?;
    assert_eq!(retained.recovered_dispatch_tenant(&lease(), &foreign), None);
    foreign = target(7)?;
    foreign.attempt_id = DispatchAttemptId::new("another-attempt")?;
    assert_eq!(retained.recovered_dispatch_tenant(&lease(), &foreign), None);
    for epoch in [
        None,
        Some(LeaseEpoch::FIRST),
        Some(LeaseEpoch::new(6)?),
        Some(LeaseEpoch::new(8)?),
    ] {
        let target = DispatchTarget { epoch, ..target(7)? };
        assert_eq!(retained.recovered_dispatch_tenant(&lease(), &target), None);
    }
    let bytes = std::fs::read_to_string(&path)?;
    assert!(!bytes.contains("fixture-request-body-value"));
    assert!(!bytes.contains("REQUEST_ONLY"));
    Ok(())
}

#[test]
fn memory_only_warn_off_and_unconfigured_verifiers_cannot_supply_recovery_authority() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    let signer = signer();
    seed(&path, &signer)?;
    for mode in [EnforcementMode::Warn, EnforcementMode::Off] {
        let weak = verifier(&signer, mode, &path)?;
        assert_eq!(weak.recovered_dispatch_tenant(&lease(), &target(7)?), None);
    }
    let unconfigured = DispatchVerifier::new(EnforcementMode::Require).with_fence_file(&path)?;
    assert_eq!(unconfigured.recovered_dispatch_tenant(&lease(), &target(7)?), None);
    let memory = DispatchVerifier::new(EnforcementMode::Require).with_key(signer.key_id(), &signer.public_key_b64())?;
    dispatch(&memory, &signer, &target(7)?)?;
    assert_eq!(memory.recovered_dispatch_tenant(&lease(), &target(7)?), None);
    Ok(())
}

#[test]
fn a_verified_assignment_change_needs_its_own_file_recovery() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    let signer = signer();
    seed(&path, &signer)?;
    let retained = verifier(&signer, EnforcementMode::Require, &path)?;
    assert!(retained.recovered_dispatch_tenant(&lease(), &target(7)?).is_some());
    dispatch(&retained, &signer, &target(8)?)?;
    assert_eq!(retained.recovered_dispatch_tenant(&lease(), &target(7)?), None);
    assert_eq!(retained.recovered_dispatch_tenant(&lease(), &target(8)?), None);
    drop(retained);
    let restored = verifier(&signer, EnforcementMode::Require, &path)?;
    assert_eq!(
        restored.recovered_dispatch_tenant(&lease(), &target(8)?),
        Some(original_tenant()?)
    );
    Ok(())
}

#[test]
fn cleaned_and_forgotten_dispatches_never_supply_recovery_authority() -> Result<(), Box<dyn Error>> {
    for cleaned in [false, true] {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("fence.jsonl");
        let signer = signer();
        seed(&path, &signer)?;
        let retained = verifier(&signer, EnforcementMode::Require, &path)?;
        if cleaned {
            cleanup(&retained, &signer, &target(7)?, 7)?;
        } else {
            retained.forget_lease(&lease());
        }
        assert_eq!(retained.recovered_dispatch_tenant(&lease(), &target(7)?), None);
        drop(retained);
        let restored = verifier(&signer, EnforcementMode::Require, &path)?;
        assert_eq!(restored.recovered_dispatch_tenant(&lease(), &target(7)?), None);
    }
    Ok(())
}

#[test]
fn incomplete_legacy_records_cannot_synthesize_worker_or_epoch_authority() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    let signer = signer();
    for value in [
        json!({"l": lease(), "t": "original-tenant", "a": "recovered-attempt", "e": 7, "c": false}),
        json!({"l": lease(), "t": "original-tenant", "w": "recovered-worker", "e": 7, "c": false}),
        json!({"l": lease(), "t": "original-tenant", "w": "recovered-worker", "a": "recovered-attempt", "c": false}),
        json!({"l": lease(), "t": "", "w": "recovered-worker", "a": "recovered-attempt", "e": 7, "c": false}),
    ] {
        std::fs::write(&path, format!("{}\n", serde_json::to_string(&value)?))?;
        let restored = verifier(&signer, EnforcementMode::Require, &path)?;
        assert_eq!(restored.recovered_dispatch_tenant(&lease(), &target(7)?), None);
    }
    Ok(())
}

#[test]
fn a_complete_record_is_recoverable_and_a_cleaned_one_is_not() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    let signer = signer();
    let record = |cleaned: bool| {
        json!({
            "l": lease(), "t": "original-tenant", "w": "recovered-worker",
            "a": "recovered-attempt", "e": 7, "c": cleaned,
        })
    };
    std::fs::write(&path, format!("{}\n", serde_json::to_string(&record(false))?))?;
    let restored = verifier(&signer, EnforcementMode::Require, &path)?;
    assert_eq!(
        restored.recovered_dispatch_tenant(&lease(), &target(7)?),
        Some(original_tenant()?)
    );
    std::fs::write(&path, format!("{}\n", serde_json::to_string(&record(true))?))?;
    let cleaned = verifier(&signer, EnforcementMode::Require, &path)?;
    assert_eq!(cleaned.recovered_dispatch_tenant(&lease(), &target(7)?), None);
    Ok(())
}

#[test]
fn an_ambiguous_journal_never_promotes_an_earlier_valid_record_to_recovery_authority() -> Result<(), Box<dyn Error>> {
    let signer = signer();
    for suffix in [b"{".as_slice(), b"{\"l\":\"recovered-lease\"", b"\xff\n", b"\n"] {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("fence.jsonl");
        seed(&path, &signer)?;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(suffix)?;
        for _ in 0..2 {
            let retained = verifier(&signer, EnforcementMode::Require, &path)?;
            assert_eq!(retained.recovered_dispatch_tenant(&lease(), &target(7)?), None);
            let body = serde_json::to_vec(&json!({"lease_id": lease()}))?;
            let scope = VerbScope {
                tenant: TenantId::new("another-tenant")?,
                lease_id: lease(),
                worker_id: Some(target(7)?.worker_id),
                attempt_id: Some(target(7)?.attempt_id),
                epoch: Some(LeaseEpoch::new(7)?),
            };
            let header = signer.sign_verb(RUN_COMMAND_SUBJECT, &body, &scope)?;
            let refused = retained.verify_verb(RUN_COMMAND_SUBJECT, &header, &body, false, SystemTime::now());
            assert!(
                matches!(refused, Err(DispatchAuthError::LeaseTenantConflict)),
                "ordinary tenant fencing must retain the earlier parsed record"
            );
        }
    }
    Ok(())
}

#[test]
fn an_unterminated_complete_record_is_insufficient_for_native_recovery() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    let signer = signer();
    seed(&path, &signer)?;
    let mut bytes = std::fs::read(&path)?;
    assert_eq!(bytes.pop(), Some(b'\n'));
    std::fs::write(&path, bytes)?;
    let retained = verifier(&signer, EnforcementMode::Require, &path)?;
    assert_eq!(retained.recovered_dispatch_tenant(&lease(), &target(7)?), None);
    Ok(())
}

#[test]
fn an_unreadable_journal_location_cannot_supply_recovery_authority() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let signer = signer();
    let result = DispatchVerifier::new(EnforcementMode::Require)
        .with_key(signer.key_id(), &signer.public_key_b64())?
        .with_fence_file(directory.path());
    assert!(result.is_err());
    Ok(())
}

#[test]
fn a_signed_worker_change_cannot_replace_a_live_recovered_attempt_owner() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    let signer = signer();
    seed(&path, &signer)?;
    let retained = verifier(&signer, EnforcementMode::Require, &path)?;
    let before = std::fs::read(&path)?;
    let mut other = target(7)?;
    other.worker_id = WorkerId::new("another-worker")?;
    let payload = serde_json::to_vec(&json!({
        "lease_id": lease(), "target": other, "request": {"tenant": "original-tenant"},
    }))?;
    let signed = signer.sign_payload(&payload, OP_LEASE_DISPATCH)?;
    let refused = retained.decode::<Value>(&signed, OP_LEASE_DISPATCH, Some(&other.worker_id));
    assert!(matches!(
        refused,
        Err(DispatchDecodeError::Auth(DispatchAuthError::WrongWorker { .. }))
    ));
    assert_eq!(std::fs::read(&path)?, before);
    assert_eq!(retained.recovered_dispatch_tenant(&lease(), &other), None);
    assert_eq!(
        retained.recovered_dispatch_tenant(&lease(), &target(7)?),
        Some(original_tenant()?)
    );
    drop(retained);
    let restored = verifier(&signer, EnforcementMode::Require, &path)?;
    assert_eq!(restored.recovered_dispatch_tenant(&lease(), &other), None);
    assert_eq!(
        restored.recovered_dispatch_tenant(&lease(), &target(7)?),
        Some(original_tenant()?)
    );
    Ok(())
}

#[test]
fn cleanup_still_allows_a_new_worker_attempt_and_epoch_for_the_same_tenant() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    let signer = signer();
    seed(&path, &signer)?;
    let retained = verifier(&signer, EnforcementMode::Require, &path)?;
    let original = target(7)?;
    cleanup(&retained, &signer, &original, 7)?;
    let next = DispatchTarget {
        worker_id: WorkerId::new("next-worker")?,
        attempt_id: DispatchAttemptId::new("next-attempt")?,
        epoch: Some(LeaseEpoch::new(8)?),
    };
    dispatch(&retained, &signer, &next)?;
    assert_eq!(retained.recovered_dispatch_tenant(&lease(), &next), None);
    drop(retained);
    let restored = verifier(&signer, EnforcementMode::Require, &path)?;
    assert_eq!(restored.recovered_dispatch_tenant(&lease(), &original), None);
    assert_eq!(
        restored.recovered_dispatch_tenant(&lease(), &next),
        Some(original_tenant()?)
    );
    Ok(())
}

#[test]
fn ambiguous_history_is_not_compacted_into_recovery_authority() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    let signer = signer();
    seed(&path, &signer)?;
    let mut history = std::fs::read(&path)?.repeat(2048);
    history.extend_from_slice(b"{\"l\":\"recovered-lease\"");
    std::fs::write(&path, &history)?;
    for _ in 0..2 {
        let restored = verifier(&signer, EnforcementMode::Require, &path)?;
        assert_eq!(restored.recovered_dispatch_tenant(&lease(), &target(7)?), None);
        assert_eq!(std::fs::read(&path)?, history);
    }
    Ok(())
}

#[test]
fn a_long_clean_history_is_compacted_without_losing_recovery_authority() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("fence.jsonl");
    let signer = signer();
    seed(&path, &signer)?;
    let history = std::fs::read(&path)?.repeat(2048);
    std::fs::write(&path, &history)?;
    let restored = verifier(&signer, EnforcementMode::Require, &path)?;
    assert_eq!(
        restored.recovered_dispatch_tenant(&lease(), &target(7)?),
        Some(original_tenant()?)
    );
    assert!(std::fs::read(&path)?.len() < history.len());
    Ok(())
}
