use std::fmt::Debug;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    DispatchAuth, ProviderDispatchReply, ProviderLease, SnapshotExportOutcome, SnapshotReceipt,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn digest() -> String {
    format!("sha256:{}", "a".repeat(64))
}

/// Parses `wire` and checks it serializes back to exactly the same JSON.
fn canonical<T>(wire: Value) -> Result<T, Box<dyn std::error::Error>>
where
    T: Debug + Serialize + DeserializeOwned,
{
    let parsed = serde_json::from_value::<T>(wire.clone())?;
    assert_eq!(serde_json::to_value(&parsed)?, wire);
    Ok(parsed)
}

#[test]
fn a_provider_lease_round_trips_with_and_without_optional_fields() -> TestResult {
    let bare = canonical::<ProviderLease>(json!({ "sandbox_id": "sbx-1", "evidence_subject": "evidence.lease-1" }))?;
    assert_eq!(bare.worker_id, None);
    assert_eq!(bare.attempt_id, None);
    assert_eq!(bare.epoch, None);
    let full = canonical::<ProviderLease>(json!({
        "sandbox_id": "sbx-1",
        "evidence_subject": "evidence.lease-1",
        "worker_id": "w1",
        "attempt_id": "a1",
        "epoch": 4,
    }))?;
    assert_eq!(full.epoch.map(|epoch| epoch.get()), Some(4));
    Ok(())
}

#[test]
fn a_provider_lease_rejects_an_empty_sandbox_id_and_a_zero_epoch() -> TestResult {
    let error = serde_json::from_value::<ProviderLease>(json!({ "sandbox_id": "", "evidence_subject": "e" }))
        .err()
        .ok_or("expected error")?;
    assert_eq!(error.to_string(), "sandbox id must not be empty");
    let error =
        serde_json::from_value::<ProviderLease>(json!({ "sandbox_id": "sbx-1", "evidence_subject": "e", "epoch": 0 }))
            .err()
            .ok_or("expected error")?;
    assert_eq!(error.to_string(), "invalid value: integer `0`, expected a nonzero u64");
    Ok(())
}

#[test]
fn dispatch_replies_are_told_apart_by_their_fields() -> TestResult {
    let granted = canonical::<ProviderDispatchReply>(json!({ "sandbox_id": "sbx-1", "evidence_subject": "e" }))?;
    assert!(matches!(granted, ProviderDispatchReply::Granted(_)));
    let failed = canonical::<ProviderDispatchReply>(json!({ "provider_error": "boom" }))?;
    assert_eq!(
        failed,
        ProviderDispatchReply::LaunchFailed {
            provider_error: "boom".to_owned()
        }
    );
    let rejected = canonical::<ProviderDispatchReply>(json!({ "decode_error": "bad json" }))?;
    assert_eq!(
        rejected,
        ProviderDispatchReply::DispatchRejected {
            decode_error: "bad json".to_owned()
        }
    );
    let native = canonical::<ProviderDispatchReply>(json!({ "native_error": "unknown key" }))?;
    assert_eq!(
        native,
        ProviderDispatchReply::NativeRejected {
            native_error: "unknown key".to_owned()
        }
    );
    Ok(())
}

#[test]
fn an_unrecognized_dispatch_reply_is_rejected() -> TestResult {
    let error = serde_json::from_value::<ProviderDispatchReply>(json!({ "unrelated": 1 }))
        .err()
        .ok_or("expected error")?;
    assert_eq!(
        error.to_string(),
        "data did not match any variant of untagged enum ProviderDispatchReply"
    );
    Ok(())
}

#[test]
fn snapshot_receipts_and_outcomes_round_trip() -> TestResult {
    let receipt_wire = json!({ "digest": digest(), "bytes": 4096, "artifact_type": "sandbox_rootfs_v1" });
    let receipt = canonical::<SnapshotReceipt>(receipt_wire.clone())?;
    assert_eq!(receipt.bytes, 4096);

    let mut ok = receipt_wire;
    ok["outcome"] = json!("ok");
    let outcome = canonical::<SnapshotExportOutcome>(ok)?;
    assert_eq!(outcome, SnapshotExportOutcome::Ok(receipt));

    let native = canonical::<SnapshotExportOutcome>(json!({
        "outcome": "native",
        "reference": { "provider": "modal", "id": "snap-1" },
        "bytes": 12,
    }))?;
    assert!(matches!(native, SnapshotExportOutcome::Native(_)));

    let failed = canonical::<SnapshotExportOutcome>(json!({
        "outcome": "err",
        "kind": "transient",
        "message": "retry later",
    }))?;
    assert!(matches!(failed, SnapshotExportOutcome::Err(_)));
    Ok(())
}

fn auth_json() -> Value {
    json!({
        "version": 1,
        "key_id": "key-1",
        "operation": "lease_dispatch",
        "tenant": "acme",
        "lease_id": "lease-1",
        "expires_at": 1_900_000_000_u64,
        "body_sha256": "b".repeat(64),
        "signature": "c2lnbmF0dXJl",
    })
}

#[test]
fn a_dispatch_auth_envelope_round_trips_without_optional_fields() -> TestResult {
    let auth = canonical::<DispatchAuth>(auth_json())?;
    assert_eq!(auth.worker_id, None);
    assert_eq!(auth.attempt_id, None);
    assert_eq!(auth.nonce, None);
    assert_eq!(auth.epoch, None);
    Ok(())
}

#[test]
fn a_dispatch_auth_envelope_round_trips_with_every_field() -> TestResult {
    let mut wire = auth_json();
    wire["worker_id"] = json!("w1");
    wire["attempt_id"] = json!("a1");
    wire["nonce"] = json!("nonce-1");
    wire["epoch"] = json!(7);
    let auth = canonical::<DispatchAuth>(wire)?;
    assert_eq!(auth.nonce.as_deref(), Some("nonce-1"));
    Ok(())
}

#[test]
fn a_dispatch_auth_envelope_rejects_a_malformed_body_digest() -> TestResult {
    let mut wire = auth_json();
    wire["body_sha256"] = json!("ABC");
    let error = serde_json::from_value::<DispatchAuth>(wire)
        .err()
        .ok_or("expected error")?;
    assert_eq!(error.to_string(), "file checksum must be exactly 64 characters");
    Ok(())
}
