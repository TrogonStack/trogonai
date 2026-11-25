use std::fmt::Debug;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    AttemptQuotaReservation, CancelIntent, DispatchAttemptId, DispatchTarget, FreezeAction, FreezeInFlight,
    IdlePauseRetry, LeaseAccounting, LeaseAttemptIdentity, LeaseDispatchAttempt, LeaseEpoch, LeaseId, LeaseReleased,
    LeaseReleasedReason, LeaseRequestKey, LeaseRequestKeyError, PendingDispatch, ProviderCleanupRequest,
    ProviderCleanupResponse, ProviderKind, TenantId, WorkerId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn round_trip<T>(value: &T, expected: &Value) -> TestResult
where
    T: Debug + PartialEq + Serialize + DeserializeOwned,
{
    assert_eq!(&serde_json::to_value(value)?, expected);
    assert_eq!(&serde_json::from_value::<T>(expected.clone())?, value);
    Ok(())
}

fn identity() -> Result<LeaseAttemptIdentity, Box<dyn std::error::Error>> {
    Ok(LeaseAttemptIdentity {
        lease_id: LeaseId::new("lease-1"),
        provider: ProviderKind::Daytona,
        target: DispatchTarget {
            worker_id: WorkerId::new("w1")?,
            attempt_id: DispatchAttemptId::new("a1")?,
            epoch: Some(LeaseEpoch::new(3)?),
        },
    })
}

fn identity_json() -> Value {
    json!({
        "lease_id": "lease-1",
        "provider": "daytona",
        "target": { "worker_id": "w1", "attempt_id": "a1", "epoch": 3 },
    })
}

fn time(secs: u64) -> std::time::SystemTime {
    std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)
}

fn time_json(secs: u64) -> Value {
    json!({ "secs_since_epoch": secs, "nanos_since_epoch": 0 })
}

#[test]
fn request_key_accepts_the_token_grammar() -> TestResult {
    let key = LeaseRequestKey::new("job-1_abc")?;
    assert_eq!(key.as_str(), "job-1_abc");
    assert_eq!(key.to_string(), "job-1_abc");
    assert_eq!("job-1_abc".parse::<LeaseRequestKey>()?, key);
    assert_eq!(LeaseRequestKey::try_from("job-1_abc".to_owned())?, key);
    assert_eq!(String::from(key.clone()), "job-1_abc");
    round_trip(&key, &json!("job-1_abc"))?;
    let longest = "k".repeat(LeaseRequestKey::MAX_LEN);
    assert_eq!(LeaseRequestKey::new(longest.clone())?.as_str(), longest);
    Ok(())
}

#[test]
fn request_key_rejects_unparsable_values() -> TestResult {
    let long = "k".repeat(LeaseRequestKey::MAX_LEN + 1);
    let cases = [
        (
            String::new(),
            LeaseRequestKeyError::Empty,
            "lease request key must not be empty",
        ),
        (
            long,
            LeaseRequestKeyError::TooLong { max: 256 },
            "lease request key must be at most 256 bytes",
        ),
        (
            "a.b".to_owned(),
            LeaseRequestKeyError::InvalidCharacter { index: 1 },
            "lease request key contains a byte outside ASCII letters, digits, '-' or '_' at index 1",
        ),
        (
            "tab\t".to_owned(),
            LeaseRequestKeyError::InvalidCharacter { index: 3 },
            "lease request key contains a byte outside ASCII letters, digits, '-' or '_' at index 3",
        ),
    ];
    for (value, expected, message) in cases {
        let error = LeaseRequestKey::new(value.clone()).err().ok_or("expected an error")?;
        assert_eq!(error, expected);
        assert_eq!(error.to_string(), message);
        assert_eq!(value.parse::<LeaseRequestKey>().err(), Some(expected));
        let error = serde_json::from_value::<LeaseRequestKey>(json!(value))
            .err()
            .ok_or("expected an error")?;
        assert_eq!(error.classify(), serde_json::error::Category::Data);
    }
    Ok(())
}

#[test]
fn cleanup_messages_round_trip() -> TestResult {
    let request = ProviderCleanupRequest {
        identity: identity()?,
        intent: CancelIntent::Purge,
    };
    round_trip(&request, &json!({ "identity": identity_json(), "intent": "purge" }))?;

    let cleaned = ProviderCleanupResponse::Cleaned {
        identity: identity()?,
        intent: CancelIntent::Release,
    };
    round_trip(
        &cleaned,
        &json!({ "outcome": "cleaned", "identity": identity_json(), "intent": "release" }),
    )?;

    let failed = ProviderCleanupResponse::Failed {
        identity: identity()?,
        message: "gone".to_owned(),
    };
    round_trip(
        &failed,
        &json!({ "outcome": "failed", "identity": identity_json(), "message": "gone" }),
    )?;
    Ok(())
}

#[test]
fn accounting_and_reservation_round_trip() -> TestResult {
    round_trip(&LeaseAccounting::Tenant, &json!("tenant"))?;
    round_trip(&LeaseAccounting::Prewarm, &json!("prewarm"))?;
    let reservation = AttemptQuotaReservation {
        tenant: TenantId::new("acme")?,
        attempt_id: DispatchAttemptId::new("a1")?,
        created_at: time(10),
        cpu_seconds: 60,
        wall_seconds: 120,
        accounting: LeaseAccounting::Prewarm,
    };
    round_trip(
        &reservation,
        &json!({
            "tenant": "acme",
            "attempt_id": "a1",
            "created_at": time_json(10),
            "cpu_seconds": 60,
            "wall_seconds": 120,
            "accounting": "prewarm",
        }),
    )
}

#[test]
fn dispatch_attempts_round_trip_with_every_field() -> TestResult {
    let attempt = LeaseDispatchAttempt {
        id: DispatchAttemptId::new("a1")?,
        epoch: Some(LeaseEpoch::new(2)?),
        idempotency_key: LeaseRequestKey::new("opaque-key")?,
        accounting: LeaseAccounting::Tenant,
        cleanup_reason: Some(LeaseReleasedReason::Failed),
        cleanup_started_at: Some(time(5)),
        pending_dispatch: Some(PendingDispatch {
            sent_at: time(4),
            payload: json!({ "lease_id": "lease-1" }),
        }),
    };
    round_trip(
        &attempt,
        &json!({
            "id": "a1",
            "epoch": 2,
            "idempotency_key": "opaque-key",
            "accounting": "tenant",
            "cleanup_reason": "failed",
            "cleanup_started_at": time_json(5),
            "pending_dispatch": { "sent_at": time_json(4), "payload": { "lease_id": "lease-1" } },
        }),
    )
}

#[test]
fn dispatch_attempts_without_optional_fields_stay_unchanged() -> TestResult {
    let legacy = json!({ "id": "attempt-1", "idempotency_key": "opaque-key", "accounting": "tenant" });
    let attempt: LeaseDispatchAttempt = serde_json::from_value(legacy.clone())?;
    assert_eq!(attempt.epoch, None);
    assert_eq!(attempt.cleanup_reason, None);
    assert_eq!(attempt.cleanup_started_at, None);
    assert_eq!(attempt.pending_dispatch, None);
    assert_eq!(serde_json::to_value(&attempt)?, legacy);
    Ok(())
}

#[test]
fn dispatch_attempts_reject_unparsable_keys() -> TestResult {
    let error = serde_json::from_value::<LeaseDispatchAttempt>(
        json!({ "id": "a1", "idempotency_key": "a b", "accounting": "tenant" }),
    )
    .err()
    .ok_or("expected an error")?;
    assert_eq!(error.classify(), serde_json::error::Category::Data);
    Ok(())
}

#[test]
fn release_reasons_round_trip() -> TestResult {
    let reasons = [
        (LeaseReleasedReason::Completed, "completed"),
        (LeaseReleasedReason::Expired, "expired"),
        (LeaseReleasedReason::Cancelled, "cancelled"),
        (LeaseReleasedReason::IdleTimeout, "idle_timeout"),
        (LeaseReleasedReason::Failed, "failed"),
    ];
    for (reason, name) in reasons {
        round_trip(&reason, &json!(name))?;
    }
    assert_eq!(LeaseReleasedReason::default(), LeaseReleasedReason::Completed);
    let error = serde_json::from_value::<LeaseReleasedReason>(json!("unknown"))
        .err()
        .ok_or("expected an error")?;
    assert_eq!(error.classify(), serde_json::error::Category::Data);
    Ok(())
}

#[test]
fn lease_released_round_trips_minimal_and_full() -> TestResult {
    let minimal = LeaseReleased {
        lease_id: LeaseId::new("lease-1"),
        tenant: TenantId::new("acme")?,
        reason: LeaseReleasedReason::Completed,
        snapshot_out: None,
        worker_id: None,
        correlation_id: None,
        restored_from: None,
        released_at: None,
    };
    round_trip(
        &minimal,
        &json!({ "lease_id": "lease-1", "tenant": "acme", "reason": "completed" }),
    )?;

    let full: LeaseReleased = serde_json::from_value(json!({
        "lease_id": "lease-1",
        "tenant": "acme",
        "reason": "idle_timeout",
        "snapshot_out": { "artifact_type": "sandbox_rootfs_v1", "target_repository": "registry.example/snaps" },
        "worker_id": "w1",
        "correlation_id": "corr-1",
        "restored_from": DIGEST,
        "released_at": time_json(9),
    }))?;
    assert_eq!(full.reason, LeaseReleasedReason::IdleTimeout);
    assert_eq!(full.worker_id.as_deref(), Some("w1"));
    assert_eq!(full.released_at, Some(time(9)));
    assert_eq!(
        serde_json::from_value::<LeaseReleased>(serde_json::to_value(&full)?)?,
        full
    );
    Ok(())
}

#[test]
fn freeze_markers_round_trip() -> TestResult {
    round_trip(&FreezeAction::Pause, &json!(false))?;
    round_trip(&FreezeAction::Resume, &json!(true))?;
    assert!(FreezeAction::Resume.resumes());
    assert!(!FreezeAction::Pause.resumes());

    let marker = FreezeInFlight {
        action: FreezeAction::Resume,
        started_at: time(7),
        settle_attempts: 2,
        next_settle_at: Some(time(8)),
    };
    round_trip(
        &marker,
        &json!({
            "thaw": true,
            "started_at": time_json(7),
            "settle_attempts": 2,
            "next_settle_at": time_json(8),
        }),
    )?;

    let bare = FreezeInFlight {
        action: FreezeAction::Pause,
        started_at: time(7),
        settle_attempts: 0,
        next_settle_at: None,
    };
    round_trip(&bare, &json!({ "thaw": false, "started_at": time_json(7) }))
}

#[test]
fn idle_pause_retry_round_trips() -> TestResult {
    let retry = IdlePauseRetry {
        failures: 3,
        retry_at: time(100),
    };
    round_trip(&retry, &json!({ "failures": 3, "retry_at": time_json(100) }))
}
