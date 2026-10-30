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

fn raw_record(
    status: Option<proto::CredentialManagementIdempotencyStatus>,
    response: Option<proto::CredentialCommandResponse>,
) -> Bytes {
    let record = proto::CredentialManagementIdempotencyRecord {
        fingerprint: "fp-1".to_string(),
        status: status.map(Into::into),
        response: response.map(MessageField::some).unwrap_or_else(MessageField::none),
    };
    Bytes::from(record.encode_to_vec())
}

fn raw_response(
    stream_position: Option<u64>,
    credential_ref: Option<proto::CredentialRefResponse>,
) -> proto::CredentialCommandResponse {
    proto::CredentialCommandResponse {
        state: "active".to_string(),
        stream_position,
        credential_ref: credential_ref
            .map(MessageField::some)
            .unwrap_or_else(MessageField::none),
    }
}

fn raw_credential_ref(version: Option<u64>) -> proto::CredentialRefResponse {
    proto::CredentialRefResponse {
        id: "id".to_string(),
        version,
        owner_id: "tenant-1".to_string(),
        source: "github".to_string(),
        scope_key: "github/primary".to_string(),
        kind: "webhook_secret".to_string(),
        status: "active".to_string(),
    }
}

fn completed_status() -> Option<proto::CredentialManagementIdempotencyStatus> {
    Some(proto::CredentialManagementIdempotencyStatus::CREDENTIAL_MANAGEMENT_IDEMPOTENCY_STATUS_COMPLETED)
}

fn kv_with_entry(
    value: Bytes,
) -> (
    MockJetStreamKvStore,
    CredentialCommandKvIdempotencyLedger<MockJetStreamKvStore>,
) {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(value, 3, kv::Operation::Put);
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv.clone());
    (kv, ledger)
}

fn test_scope() -> IdempotencyScope {
    scope("tenant-1", "target", "retry-1")
}

fn is_codec(error: &CredentialCommandIdempotencyStoreError, reason: &str) -> bool {
    matches!(error, CredentialCommandIdempotencyStoreError::Codec { source } if source.to_string() == reason)
}

#[test]
fn idempotency_key_rejects_empty_value() {
    assert!(matches!(IdempotencyKey::new(""), Err(IdempotencyKeyError::Empty)));
}

#[test]
fn idempotency_key_rejects_values_beyond_the_maximum_length() {
    assert!(IdempotencyKey::new("a".repeat(256)).is_ok());
    assert!(matches!(
        IdempotencyKey::new("a".repeat(257)),
        Err(IdempotencyKeyError::TooLong(257))
    ));
}

#[tokio::test]
async fn in_memory_ledger_complete_without_a_started_record_is_a_no_op() {
    let ledger = CredentialCommandInMemoryIdempotencyLedger::default();

    ledger.complete(&test_scope(), response(1)).await.unwrap();
    let decision = ledger.begin(test_scope(), fingerprint("fp-1")).await.unwrap();

    assert!(matches!(decision, IdempotencyDecision::Execute));
}

#[tokio::test]
async fn kv_ledger_begin_creates_record_when_entry_is_missing_then_retries_after_race() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    kv.enqueue_create_result(Err(kv::CreateErrorKind::AlreadyExists));
    kv.enqueue_entry(
        Bytes::from(encode_record(&StoredIdempotencyRecord::completed(
            fingerprint("fp-1"),
            response(1),
        ))),
        4,
        kv::Operation::Put,
    );
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv.clone());

    let decision = ledger.begin(test_scope(), fingerprint("fp-1")).await.unwrap();

    assert!(matches!(decision, IdempotencyDecision::Replay(_)));
    assert_eq!(kv.create_calls().len(), 1);
}

#[tokio::test]
async fn kv_ledger_begin_surfaces_create_failures_for_missing_entry() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    kv.enqueue_create_result(Err(kv::CreateErrorKind::Other));
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv);

    let error = ledger.begin(test_scope(), fingerprint("fp-1")).await.unwrap_err();

    assert!(matches!(error, CredentialCommandIdempotencyStoreError::Backend { .. }));
}

#[tokio::test]
async fn kv_ledger_begin_recreates_record_over_deleted_entry() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(Bytes::new(), 4, kv::Operation::Delete);
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv.clone());

    let decision = ledger.begin(test_scope(), fingerprint("fp-1")).await.unwrap();

    assert!(matches!(decision, IdempotencyDecision::Execute));
    assert_eq!(kv.create_calls().len(), 1);
}

#[tokio::test]
async fn kv_ledger_begin_retries_when_recreate_over_purged_entry_races() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(Bytes::new(), 4, kv::Operation::Purge);
    kv.enqueue_create_result(Err(kv::CreateErrorKind::AlreadyExists));
    kv.enqueue_entry_none();
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv.clone());

    let decision = ledger.begin(test_scope(), fingerprint("fp-1")).await.unwrap();

    assert!(matches!(decision, IdempotencyDecision::Execute));
    assert_eq!(kv.create_calls().len(), 2);
}

#[tokio::test]
async fn kv_ledger_begin_surfaces_create_failures_over_deleted_entry() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(Bytes::new(), 4, kv::Operation::Delete);
    kv.enqueue_create_result(Err(kv::CreateErrorKind::Other));
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv);

    let error = ledger.begin(test_scope(), fingerprint("fp-1")).await.unwrap_err();

    assert!(matches!(error, CredentialCommandIdempotencyStoreError::Backend { .. }));
}

#[tokio::test]
async fn kv_ledger_begin_surfaces_reclaim_update_failures() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(
        Bytes::from(encode_record(&StoredIdempotencyRecord::in_progress(fingerprint(
            "fp-1",
        )))),
        5,
        kv::Operation::Put,
    );
    kv.enqueue_update_result(Err(kv::UpdateErrorKind::Other));
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv).with_in_progress_lease(Duration::ZERO);

    let error = ledger.begin(test_scope(), fingerprint("fp-1")).await.unwrap_err();

    assert!(matches!(error, CredentialCommandIdempotencyStoreError::Backend { .. }));
}

#[tokio::test]
async fn kv_ledger_begin_surfaces_entry_lookup_failures() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_error(kv::EntryErrorKind::Other);
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv);

    let error = ledger.begin(test_scope(), fingerprint("fp-1")).await.unwrap_err();

    assert!(matches!(error, CredentialCommandIdempotencyStoreError::Backend { .. }));
}

#[tokio::test]
async fn kv_ledger_begin_reports_conflict_after_exhausting_retries() {
    let kv = MockJetStreamKvStore::new();
    for revision in 1..=3 {
        kv.enqueue_entry(
            Bytes::from(encode_record(&StoredIdempotencyRecord::in_progress(fingerprint(
                "fp-1",
            )))),
            revision,
            kv::Operation::Put,
        );
        kv.enqueue_update_result(Err(kv::UpdateErrorKind::WrongLastRevision));
    }
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv.clone()).with_in_progress_lease(Duration::ZERO);

    let error = ledger.begin(test_scope(), fingerprint("fp-1")).await.unwrap_err();

    assert!(matches!(error, CredentialCommandIdempotencyStoreError::Conflict));
    assert_eq!(kv.update_calls().len(), 3);
}

#[tokio::test]
async fn kv_ledger_complete_conflicts_when_entry_is_missing() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv.clone());

    let error = ledger.complete(&test_scope(), response(1)).await.unwrap_err();

    assert!(matches!(error, CredentialCommandIdempotencyStoreError::Conflict));
    assert!(kv.update_calls().is_empty());
}

#[tokio::test]
async fn kv_ledger_complete_conflicts_when_entry_was_deleted() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(Bytes::new(), 4, kv::Operation::Delete);
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv.clone());

    let error = ledger.complete(&test_scope(), response(1)).await.unwrap_err();

    assert!(matches!(error, CredentialCommandIdempotencyStoreError::Conflict));
    assert!(kv.update_calls().is_empty());
}

#[tokio::test]
async fn kv_ledger_complete_maps_revision_mismatch_to_conflict() {
    let (kv, ledger) = kv_with_entry(Bytes::from(encode_record(&StoredIdempotencyRecord::in_progress(
        fingerprint("fp-1"),
    ))));
    kv.enqueue_update_result(Err(kv::UpdateErrorKind::WrongLastRevision));

    let error = ledger.complete(&test_scope(), response(1)).await.unwrap_err();

    assert!(matches!(error, CredentialCommandIdempotencyStoreError::Conflict));
}

#[tokio::test]
async fn kv_ledger_complete_surfaces_other_update_failures() {
    let (kv, ledger) = kv_with_entry(Bytes::from(encode_record(&StoredIdempotencyRecord::in_progress(
        fingerprint("fp-1"),
    ))));
    kv.enqueue_update_result(Err(kv::UpdateErrorKind::Other));

    let error = ledger.complete(&test_scope(), response(1)).await.unwrap_err();

    assert!(matches!(error, CredentialCommandIdempotencyStoreError::Backend { .. }));
}

#[tokio::test]
async fn kv_ledger_abandon_is_a_no_op_when_entry_is_missing() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv.clone());

    ledger.abandon(&test_scope()).await.unwrap();

    assert!(kv.delete_calls().is_empty());
}

#[tokio::test]
async fn kv_ledger_abandon_is_a_no_op_when_entry_was_deleted() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(Bytes::new(), 4, kv::Operation::Purge);
    let ledger = CredentialCommandKvIdempotencyLedger::new(kv.clone());

    ledger.abandon(&test_scope()).await.unwrap();

    assert!(kv.delete_calls().is_empty());
}

#[tokio::test]
async fn kv_ledger_abandon_maps_revision_mismatch_to_conflict() {
    let (kv, ledger) = kv_with_entry(Bytes::from(encode_record(&StoredIdempotencyRecord::in_progress(
        fingerprint("fp-1"),
    ))));
    kv.enqueue_delete_result(Err(kv::DeleteErrorKind::WrongLastRevision));

    let error = ledger.abandon(&test_scope()).await.unwrap_err();

    assert!(matches!(error, CredentialCommandIdempotencyStoreError::Conflict));
}

#[tokio::test]
async fn kv_ledger_abandon_surfaces_other_delete_failures() {
    let (kv, ledger) = kv_with_entry(Bytes::from(encode_record(&StoredIdempotencyRecord::in_progress(
        fingerprint("fp-1"),
    ))));
    kv.enqueue_delete_result(Err(kv::DeleteErrorKind::Other));

    let error = ledger.abandon(&test_scope()).await.unwrap_err();

    assert!(matches!(error, CredentialCommandIdempotencyStoreError::Backend { .. }));
}

#[tokio::test]
async fn kv_ledger_rejects_completed_record_without_response() {
    let (_, ledger) = kv_with_entry(raw_record(completed_status(), None));

    let error = ledger.begin(test_scope(), fingerprint("fp-1")).await.unwrap_err();

    assert!(is_codec(&error, "completed idempotency record is missing response"));
}

#[tokio::test]
async fn kv_ledger_rejects_record_without_status() {
    let (_, ledger) = kv_with_entry(raw_record(None, None));

    let error = ledger.begin(test_scope(), fingerprint("fp-1")).await.unwrap_err();

    assert!(is_codec(&error, "idempotency record is missing status"));
}

#[tokio::test]
async fn kv_ledger_rejects_record_with_unknown_status() {
    let record = proto::CredentialManagementIdempotencyRecord {
        fingerprint: "fp-1".to_string(),
        status: Some(EnumValue::Unknown(99)),
        response: MessageField::none(),
    };
    let (_, ledger) = kv_with_entry(Bytes::from(record.encode_to_vec()));

    let error = ledger.begin(test_scope(), fingerprint("fp-1")).await.unwrap_err();

    assert!(is_codec(&error, "idempotency record has unknown status"));
}

#[tokio::test]
async fn kv_ledger_rejects_record_with_unspecified_status() {
    let status = proto::CredentialManagementIdempotencyStatus::CREDENTIAL_MANAGEMENT_IDEMPOTENCY_STATUS_UNSPECIFIED;
    let (_, ledger) = kv_with_entry(raw_record(Some(status), None));

    let error = ledger.begin(test_scope(), fingerprint("fp-1")).await.unwrap_err();

    assert!(is_codec(&error, "idempotency record has unspecified status"));
}

#[tokio::test]
async fn kv_ledger_rejects_response_without_credential_ref() {
    let (_, ledger) = kv_with_entry(raw_record(completed_status(), Some(raw_response(Some(2), None))));

    let error = ledger.begin(test_scope(), fingerprint("fp-1")).await.unwrap_err();

    assert!(is_codec(
        &error,
        "completed idempotency response is missing credential ref"
    ));
}

#[tokio::test]
async fn kv_ledger_rejects_response_without_stream_position() {
    let response = raw_response(None, Some(raw_credential_ref(Some(1))));
    let (_, ledger) = kv_with_entry(raw_record(completed_status(), Some(response)));

    let error = ledger.begin(test_scope(), fingerprint("fp-1")).await.unwrap_err();

    assert!(is_codec(
        &error,
        "completed idempotency response is missing stream position"
    ));
}

#[tokio::test]
async fn kv_ledger_rejects_credential_ref_without_version() {
    let response = raw_response(Some(2), Some(raw_credential_ref(None)));
    let (_, ledger) = kv_with_entry(raw_record(completed_status(), Some(response)));

    let error = ledger.begin(test_scope(), fingerprint("fp-1")).await.unwrap_err();

    assert!(is_codec(
        &error,
        "completed idempotency response is missing credential ref version"
    ));
}

#[tokio::test]
async fn kv_ledger_rejects_undecodable_record_bytes() {
    let (_, ledger) = kv_with_entry(Bytes::from_static(&[0xff, 0xff, 0xff]));

    let error = ledger.begin(test_scope(), fingerprint("fp-1")).await.unwrap_err();

    assert!(matches!(error, CredentialCommandIdempotencyStoreError::Codec { .. }));
}

#[test]
fn store_errors_render_their_source() {
    let backend = CredentialCommandIdempotencyStoreError::backend(std::io::Error::other("down"));

    assert_eq!(
        backend.to_string(),
        "credential command idempotency backend failed: down"
    );
}

#[tokio::test]
async fn open_provisions_the_bucket_and_serves_a_working_ledger() {
    let server = trogon_nats::test_support::JetStreamTestServer::start().await;
    let ledger = open(server.jetstream().await).await.unwrap();
    let reopened = open(server.jetstream().await).await.unwrap();

    let first = ledger
        .begin(scope("tenant-1", "target", "retry-1"), fingerprint("fp-1"))
        .await
        .unwrap();
    ledger
        .complete(&scope("tenant-1", "target", "retry-1"), response(1))
        .await
        .unwrap();
    let replayed = reopened
        .begin(scope("tenant-1", "target", "retry-1"), fingerprint("fp-1"))
        .await
        .unwrap();

    assert!(matches!(first, IdempotencyDecision::Execute));
    assert!(matches!(replayed, IdempotencyDecision::Replay(replayed) if replayed.credential_ref.version == 1));
}
