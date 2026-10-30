use async_nats::jetstream::context::{CreateKeyValueErrorKind, KeyValueErrorKind};
use trogon_nats::jetstream::{MockJetStreamKvClient, MockJetStreamKvStore};

use super::*;
use crate::credential_management::{CredentialCommandResponse, CredentialRefResponse};

fn scope(owner: &str, target: &str, key: &str) -> IdempotencyScope {
    IdempotencyScope::new(owner, "credential.create", target, IdempotencyKey::new(key).unwrap())
}

fn fingerprint(value: &str) -> RequestFingerprint {
    RequestFingerprint::new(value)
}

fn response(version: u64) -> CredentialCommandResponse {
    CredentialCommandResponse {
        state: "active".to_string(),
        stream_position: 2,
        credential_ref: CredentialRefResponse {
            id: "openbao:tenant-1:github/primary:webhook_secret".to_string(),
            version,
            owner_id: "tenant-1".to_string(),
            source: "github".to_string(),
            scope_key: "github/primary".to_string(),
            kind: "webhook_secret".to_string(),
            status: "active".to_string(),
        },
    }
}

#[tokio::test]
async fn kv_ledger_creates_in_progress_then_replays_completed_response() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    kv.enqueue_entry(
        Bytes::from(encode_record(&StoredIdempotencyRecord::in_progress(fingerprint(
            "fp-1",
        )))),
        1,
        kv::Operation::Put,
    );
    kv.enqueue_entry(
        Bytes::from(encode_record(&StoredIdempotencyRecord::completed(
            fingerprint("fp-1"),
            response(1),
        ))),
        2,
        kv::Operation::Put,
    );
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv.clone());
    let scope = scope("tenant-1", "openbao:tenant-1:github/primary:webhook_secret", "retry-1");

    let first = ledger.begin(scope.clone(), fingerprint("fp-1")).await.unwrap();
    ledger.complete(&scope, response(1)).await.unwrap();
    let second = ledger.begin(scope, fingerprint("fp-1")).await.unwrap();

    assert!(matches!(first, IdempotencyDecision::Execute));
    assert!(matches!(second, IdempotencyDecision::Replay(_)));
    assert_eq!(kv.create_calls().len(), 1);
    assert_eq!(kv.update_calls().len(), 1);
}

#[tokio::test]
async fn kv_ledger_conflicts_when_fingerprint_differs() {
    let record = StoredIdempotencyRecord::completed(fingerprint("fp-1"), response(1));
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(Bytes::from(encode_record(&record)), 2, kv::Operation::Put);
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv);

    let decision = ledger
        .begin(
            scope("tenant-1", "openbao:tenant-1:github/primary:webhook_secret", "retry-1"),
            fingerprint("fp-2"),
        )
        .await
        .unwrap();

    assert!(matches!(decision, IdempotencyDecision::Conflict));
}

#[tokio::test]
async fn kv_ledger_abandon_deletes_in_progress_record() {
    let record = StoredIdempotencyRecord::in_progress(fingerprint("fp-1"));
    let kv = MockJetStreamKvStore::new();
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv.clone());
    let scope = scope("tenant-1", "openbao:tenant-1:github/primary:webhook_secret", "retry-1");
    let key = scope.storage_key();
    kv.enqueue_entry(Bytes::from(encode_record(&record)), 7, kv::Operation::Put);

    ledger.abandon(&scope).await.unwrap();

    assert_eq!(kv.delete_calls(), vec![(key, Some(7))]);
}

#[tokio::test]
async fn kv_ledger_abandon_keeps_completed_record() {
    let record = StoredIdempotencyRecord::completed(fingerprint("fp-1"), response(1));
    let kv = MockJetStreamKvStore::new();
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv.clone());
    let scope = scope("tenant-1", "openbao:tenant-1:github/primary:webhook_secret", "retry-1");
    kv.enqueue_entry(Bytes::from(encode_record(&record)), 7, kv::Operation::Put);

    ledger.abandon(&scope).await.unwrap();

    assert!(kv.delete_calls().is_empty());
}

#[test]
fn bucket_config_matches_runtime_contract() {
    let config = bucket_config();

    assert_eq!(config.bucket, CREDENTIAL_MANAGEMENT_IDEMPOTENCY_BUCKET);
    assert_eq!(config.history, 1);
    assert_eq!(config.max_age, CREDENTIAL_MANAGEMENT_IDEMPOTENCY_MAX_AGE);
}

#[tokio::test]
async fn provision_bucket_falls_back_to_get_on_already_exists() {
    let client = MockJetStreamKvClient::new();
    client.fail_create_already_exists();
    let store = MockJetStreamKvStore::new();
    store.set_settings(1, CREDENTIAL_MANAGEMENT_IDEMPOTENCY_MAX_AGE, true, None);
    client.set_get_result(store);

    let result = provision_bucket::<_, MockJetStreamKvStore>(&client).await;

    assert!(result.is_ok());
    assert_eq!(
        client.requested_buckets(),
        vec![CREDENTIAL_MANAGEMENT_IDEMPOTENCY_BUCKET.to_string()]
    );
}

#[tokio::test]
async fn provision_bucket_surfaces_get_errors_after_already_exists() {
    let client = MockJetStreamKvClient::new();
    client.fail_create_already_exists();
    client.fail_get(KeyValueErrorKind::GetBucket);

    let error = provision_bucket::<_, MockJetStreamKvStore>(&client).await.unwrap_err();

    assert!(matches!(
        error,
        CredentialManagementIdempotencyOpenError::OpenExisting(_)
    ));
}

#[tokio::test]
async fn provision_bucket_surfaces_create_errors() {
    let client = MockJetStreamKvClient::new();
    client.fail_create(CreateKeyValueErrorKind::TimedOut);

    let error = provision_bucket::<_, MockJetStreamKvStore>(&client).await.unwrap_err();

    assert!(matches!(error, CredentialManagementIdempotencyOpenError::Create(_)));
}

#[tokio::test]
async fn provision_bucket_rejects_incompatible_max_age() {
    let client = MockJetStreamKvClient::new();
    let store = MockJetStreamKvStore::new();
    store.set_settings(1, Duration::ZERO, true, None);
    client.set_create_result(store);

    let error = provision_bucket::<_, MockJetStreamKvStore>(&client).await.unwrap_err();

    assert!(matches!(
        error,
        CredentialManagementIdempotencyOpenError::Incompatible {
            source: IncompatibleCredentialManagementIdempotencyBucketError {
                expected_max_age,
                actual_max_age,
                ..
            },
        } if expected_max_age == CREDENTIAL_MANAGEMENT_IDEMPOTENCY_MAX_AGE && actual_max_age == Duration::ZERO
    ));
}

const LONG_LEASE: Duration = Duration::from_secs(100 * 365 * 24 * 60 * 60);

#[tokio::test(start_paused = true)]
async fn in_memory_ledger_reports_in_progress_within_lease() {
    let ledger = CredentialCommandInMemoryIdempotencyLedger::with_in_progress_lease(Duration::from_secs(60));
    let scope = scope("tenant-1", "target", "retry-1");

    let first = ledger.begin(scope.clone(), fingerprint("fp-1")).await.unwrap();
    tokio::time::advance(Duration::from_secs(59)).await;
    let second = ledger.begin(scope, fingerprint("fp-1")).await.unwrap();

    assert!(matches!(first, IdempotencyDecision::Execute));
    assert!(matches!(second, IdempotencyDecision::InProgress));
}

#[tokio::test(start_paused = true)]
async fn in_memory_ledger_reclaims_expired_in_progress_record() {
    let ledger = CredentialCommandInMemoryIdempotencyLedger::with_in_progress_lease(Duration::from_secs(60));
    let scope = scope("tenant-1", "target", "retry-1");

    ledger.begin(scope.clone(), fingerprint("fp-1")).await.unwrap();
    tokio::time::advance(Duration::from_secs(60)).await;
    let reclaimed = ledger.begin(scope.clone(), fingerprint("fp-1")).await.unwrap();
    let after_reclaim = ledger.begin(scope, fingerprint("fp-1")).await.unwrap();

    assert!(matches!(reclaimed, IdempotencyDecision::Execute));
    assert!(matches!(after_reclaim, IdempotencyDecision::InProgress));
}

#[tokio::test(start_paused = true)]
async fn in_memory_ledger_keeps_conflict_for_expired_record_with_other_fingerprint() {
    let ledger = CredentialCommandInMemoryIdempotencyLedger::with_in_progress_lease(Duration::from_secs(60));
    let scope = scope("tenant-1", "target", "retry-1");

    ledger.begin(scope.clone(), fingerprint("fp-1")).await.unwrap();
    tokio::time::advance(Duration::from_secs(120)).await;
    let decision = ledger.begin(scope, fingerprint("fp-2")).await.unwrap();

    assert!(matches!(decision, IdempotencyDecision::Conflict));
}

#[tokio::test]
async fn kv_ledger_reclaims_expired_in_progress_record_with_revision_check() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(
        Bytes::from(encode_record(&StoredIdempotencyRecord::in_progress(fingerprint(
            "fp-1",
        )))),
        5,
        kv::Operation::Put,
    );
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv.clone()).with_in_progress_lease(Duration::from_secs(60));
    let scope = scope("tenant-1", "target", "retry-1");
    let key = scope.storage_key();

    let decision = ledger.begin(scope, fingerprint("fp-1")).await.unwrap();

    assert!(matches!(decision, IdempotencyDecision::Execute));
    let updates = kv.update_calls();
    assert_eq!(updates.len(), 1);
    assert_eq!((updates[0].0.as_str(), updates[0].2), (key.as_str(), 5));
    assert!(kv.create_calls().is_empty());
}

#[tokio::test]
async fn kv_ledger_reports_in_progress_within_lease() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(
        Bytes::from(encode_record(&StoredIdempotencyRecord::in_progress(fingerprint(
            "fp-1",
        )))),
        5,
        kv::Operation::Put,
    );
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv.clone()).with_in_progress_lease(LONG_LEASE);

    let decision = ledger
        .begin(scope("tenant-1", "target", "retry-1"), fingerprint("fp-1"))
        .await
        .unwrap();

    assert!(matches!(decision, IdempotencyDecision::InProgress));
    assert!(kv.update_calls().is_empty());
}

#[tokio::test]
async fn kv_ledger_retries_when_reclaim_loses_the_race() {
    let kv = MockJetStreamKvStore::new();
    let stale = Bytes::from(encode_record(&StoredIdempotencyRecord::in_progress(fingerprint(
        "fp-1",
    ))));
    kv.enqueue_entry(stale, 5, kv::Operation::Put);
    kv.enqueue_update_result(Err(kv::UpdateErrorKind::WrongLastRevision));
    kv.enqueue_entry(
        Bytes::from(encode_record(&StoredIdempotencyRecord::completed(
            fingerprint("fp-1"),
            response(1),
        ))),
        6,
        kv::Operation::Put,
    );
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv).with_in_progress_lease(Duration::from_secs(60));

    let decision = ledger
        .begin(scope("tenant-1", "target", "retry-1"), fingerprint("fp-1"))
        .await
        .unwrap();

    assert!(matches!(decision, IdempotencyDecision::Replay(_)));
}
