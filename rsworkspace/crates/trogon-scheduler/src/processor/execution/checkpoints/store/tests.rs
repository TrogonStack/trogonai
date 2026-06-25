use std::time::Duration;

use async_nats::jetstream::context::{PublishError, PublishErrorKind};
use chrono::Utc;
use trogon_decider_runtime::StreamPosition;
use trogon_nats::jetstream::MockJetStreamKvStore;

use super::*;
use crate::commands::domain::{Delivery, MessageContent, Schedule, ScheduleHeaders, ScheduleId, ScheduleMessage};
use crate::processor::execution::checkpoints::corrupt_checkpoint_schedule;
use crate::processor::execution::checkpoints::{ReconcileOutcome, ScheduleStatus};

fn record(id: &str) -> ScheduleCheckpointRecord {
    let schedule_id = ScheduleId::parse(id).unwrap();
    ScheduleCheckpointRecord {
        schedule_id,
        status: ScheduleStatus::Scheduled,
        schedule: Schedule::every(Duration::from_secs(30)).unwrap(),
        delivery: Delivery::nats_event("agent.run").unwrap(),
        message: ScheduleMessage {
            content: MessageContent::json("{}"),
            headers: ScheduleHeaders::default(),
        },
        last_applied_stream_position: StreamPosition::try_new(1).unwrap(),
        last_applied_event_id: Some("event-1".to_string()),
        last_outcome: ReconcileOutcome::Published,
    }
}
#[tokio::test]
async fn save_creates_when_no_revision_and_updates_when_present() {
    let kv = MockJetStreamKvStore::new();
    let store = ScheduleCheckpointStore::new(kv.clone());
    let record = record("orders");

    store.save(&record, None).await.unwrap();
    assert_eq!(kv.create_calls().len(), 1);
    assert_eq!(kv.update_calls().len(), 0);
    assert!(kv.create_calls()[0].0.starts_with("v1."));

    store.save(&record, Some(3)).await.unwrap();
    assert_eq!(kv.update_calls().len(), 1);
    assert_eq!(kv.update_calls()[0].2, 3);
}

#[tokio::test]
async fn save_update_cas_loss_is_a_conflict() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_update_result(Err(kv::UpdateErrorKind::WrongLastRevision));
    let store = ScheduleCheckpointStore::new(kv);

    let error = store.save(&record("orders"), Some(3)).await.unwrap_err();

    assert!(matches!(error, CheckpointStoreError::Conflict));
    assert!(!error.is_transient());
}

#[tokio::test]
async fn save_create_already_exists_is_a_conflict() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_create_result(Err(kv::CreateErrorKind::AlreadyExists));
    let store = ScheduleCheckpointStore::new(kv);

    let error = store.save(&record("orders"), None).await.unwrap_err();

    assert!(matches!(error, CheckpointStoreError::Conflict));
    assert!(!error.is_transient());
}

#[tokio::test]
async fn load_decodes_entry_and_reports_revision() {
    let kv = MockJetStreamKvStore::new();
    let record = record("orders");
    let encoded = Bytes::from(encode_checkpoint_record(&record).unwrap());
    kv.enqueue_entry(encoded, 9, kv::Operation::Put);

    let store = ScheduleCheckpointStore::new(kv);
    let loaded = store.load(&record.key()).await.unwrap().unwrap();
    assert_eq!(loaded.revision, 9);
    assert_eq!(loaded.record, record);
}

#[tokio::test]
async fn deleted_entries_are_absent() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(Bytes::new(), 4, kv::Operation::Delete);
    let store = ScheduleCheckpointStore::new(kv);
    let loaded = store
        .load(&ScheduleKey::derive(&ScheduleId::parse("orders").unwrap()))
        .await
        .unwrap();
    assert!(loaded.is_none());
}

#[tokio::test]
async fn missing_entry_is_absent() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_none();
    let store = ScheduleCheckpointStore::new(kv);
    let loaded = store
        .load(&ScheduleKey::derive(&ScheduleId::parse("orders").unwrap()))
        .await
        .unwrap();
    assert!(loaded.is_none());
}

#[tokio::test]
async fn load_by_subject_reads_the_same_key_space() {
    let kv = MockJetStreamKvStore::new();
    let record = record("orders");
    kv.enqueue_entry(
        Bytes::from(encode_checkpoint_record(&record).unwrap()),
        7,
        kv::Operation::Put,
    );
    let store = ScheduleCheckpointStore::new(kv.clone());

    let loaded = store.load_by_subject(&record.subject()).await.unwrap().unwrap();
    assert_eq!(loaded.record.schedule_id, record.schedule_id);
    assert_eq!(loaded.revision, 7);
    assert_eq!(kv.entry_calls()[0], format!("v1.{}", record.key().simple()));
}

#[tokio::test]
async fn load_by_subject_revision_supports_optimistic_save() {
    let kv = MockJetStreamKvStore::new();
    let record = record("orders");
    kv.enqueue_entry(
        Bytes::from(encode_checkpoint_record(&record).unwrap()),
        11,
        kv::Operation::Put,
    );
    let store = ScheduleCheckpointStore::new(kv.clone());

    let loaded = store.load_by_subject(&record.subject()).await.unwrap().unwrap();
    store.save(&loaded.record, Some(loaded.revision)).await.unwrap();

    assert!(kv.create_calls().is_empty());
    assert_eq!(kv.update_calls().len(), 1);
    assert_eq!(kv.update_calls()[0].2, 11);
}

#[tokio::test]
async fn load_by_id_reads_the_derived_key_space() {
    let kv = MockJetStreamKvStore::new();
    let record = record("orders");
    kv.enqueue_entry(
        Bytes::from(encode_checkpoint_record(&record).unwrap()),
        3,
        kv::Operation::Put,
    );
    let store = ScheduleCheckpointStore::new(kv.clone());

    let loaded = store.load_by_id(&record.schedule_id).await.unwrap().unwrap();

    assert_eq!(loaded.record.schedule_id, record.schedule_id);
    assert_eq!(kv.entry_calls()[0], format!("v1.{}", record.key().simple()));
}

#[tokio::test]
async fn load_by_subject_missing_entry_is_absent() {
    let kv = MockJetStreamKvStore::new();
    let record = record("orders");
    kv.enqueue_entry_none();
    let store = ScheduleCheckpointStore::new(kv);

    let loaded = store.load_by_subject(&record.subject()).await.unwrap();

    assert!(loaded.is_none());
}

#[tokio::test]
async fn load_by_subject_deleted_entry_is_absent() {
    let kv = MockJetStreamKvStore::new();
    let record = record("orders");
    kv.enqueue_entry(Bytes::new(), 4, kv::Operation::Purge);
    let store = ScheduleCheckpointStore::new(kv);

    let loaded = store.load_by_subject(&record.subject()).await.unwrap();

    assert!(loaded.is_none());
}

#[tokio::test]
async fn list_skips_failure_and_non_checkpoint_keys() {
    let kv = MockJetStreamKvStore::new();
    let record = record("orders");
    kv.set_keys_result(Ok(vec![
        format!("v1.{}", record.key().simple()),
        "failure.v1.STREAM.7".to_string(),
    ]));
    kv.enqueue_get_some(Bytes::from(encode_checkpoint_record(&record).unwrap()));
    let store = ScheduleCheckpointStore::new(kv.clone());

    let records = store.list().await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].schedule_id, record.schedule_id);
    // Only the checkpoint key triggers a value read.
    assert_eq!(kv.get_calls(), vec![format!("v1.{}", record.key().simple())]);
}

#[tokio::test]
async fn list_skips_corrupt_checkpoint_records() {
    let kv = MockJetStreamKvStore::new();
    let record = record("orders");
    let healthy_key = format!("v1.{}", record.key().simple());
    kv.set_keys_result(Ok(vec!["v1.corrupt".to_string(), healthy_key.clone()]));
    kv.enqueue_get_some(Bytes::from_static(b"not proto"));
    kv.enqueue_get_some(Bytes::from(encode_checkpoint_record(&record).unwrap()));
    let store = ScheduleCheckpointStore::new(kv.clone());

    let records = store.list().await.unwrap();

    assert_eq!(records, vec![record]);
    assert_eq!(kv.get_calls(), vec!["v1.corrupt".to_string(), healthy_key]);
}

#[tokio::test]
async fn record_failure_treats_already_exists_as_success() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_create_result(Err(kv::CreateErrorKind::AlreadyExists));
    let store = ScheduleCheckpointStore::new(kv);

    let failure = ProcessingFailureRecord::new(
        "SCHEDULER_SCHEDULE_EVENTS",
        StreamPosition::try_new(7).unwrap(),
        None,
        "boom",
        Utc::now(),
    );
    store.record_failure(&failure).await.unwrap();
}

#[tokio::test]
async fn record_failure_backend_error_is_transient() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_create_result(Err(kv::CreateErrorKind::Other));
    let store = ScheduleCheckpointStore::new(kv);

    let failure = ProcessingFailureRecord::new(
        "SCHEDULER_SCHEDULE_EVENTS",
        StreamPosition::try_new(7).unwrap(),
        None,
        "boom",
        Utc::now(),
    );
    let error = store.record_failure(&failure).await.unwrap_err();

    assert!(error.is_transient());
    assert_eq!(error.to_string(), "scheduler checkpoint backend failed: other error");
    assert!(std::error::Error::source(&error).is_some());
}

#[tokio::test]
async fn backend_errors_are_transient() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_error(kv::EntryErrorKind::Other);
    let store = ScheduleCheckpointStore::new(kv);
    let error = store
        .load(&ScheduleKey::derive(&ScheduleId::parse("orders").unwrap()))
        .await
        .unwrap_err();
    assert!(error.is_transient());
}

#[tokio::test]
async fn invalid_key_backend_errors_are_permanent() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_error(kv::EntryErrorKind::InvalidKey);
    let store = ScheduleCheckpointStore::new(kv);
    let error = store
        .load(&ScheduleKey::derive(&ScheduleId::parse("orders").unwrap()))
        .await
        .unwrap_err();

    assert!(matches!(error, CheckpointStoreError::PermanentBackend { .. }));
    assert!(!error.is_transient());
}

#[test]
fn permanent_publish_errors_in_the_source_chain_are_not_transient() {
    for kind in [PublishErrorKind::StreamNotFound, PublishErrorKind::MaxPayloadExceeded] {
        let update = kv::UpdateError::with_source(kv::UpdateErrorKind::Other, PublishError::new(kind));
        let error = CheckpointStoreError::backend(update);
        assert!(matches!(error, CheckpointStoreError::PermanentBackend { .. }));
        assert!(!error.is_transient());
    }

    let update = kv::UpdateError::with_source(
        kv::UpdateErrorKind::Other,
        PublishError::new(PublishErrorKind::TimedOut),
    );
    assert!(CheckpointStoreError::backend(update).is_transient());
}

#[tokio::test]
async fn corrupt_stored_bytes_are_a_codec_error() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry(Bytes::from_static(b"not proto"), 1, kv::Operation::Put);
    let store = ScheduleCheckpointStore::new(kv);
    let error = store
        .load(&ScheduleKey::derive(&ScheduleId::parse("orders").unwrap()))
        .await
        .unwrap_err();
    assert!(!error.is_transient());
    assert_eq!(error.corrupt_revision(), Some(1));
}

#[tokio::test]
async fn corrupt_stored_bytes_capture_watermark_when_envelope_parses() {
    let kv = MockJetStreamKvStore::new();
    let record = record("orders");
    let corrupt = corrupt_checkpoint_schedule(&encode_checkpoint_record(&record).unwrap());
    kv.enqueue_entry(Bytes::from(corrupt), 1, kv::Operation::Put);
    let store = ScheduleCheckpointStore::new(kv);
    let error = store
        .load(&ScheduleKey::derive(&ScheduleId::parse("orders").unwrap()))
        .await
        .unwrap_err();

    assert!(!error.is_transient());
    assert_eq!(error.corrupt_revision(), Some(1));
    assert_eq!(error.corrupt_watermark(), Some(StreamPosition::try_new(1).unwrap()));
    assert_eq!(error.corrupt_last_applied_event_id(), Some("event-1"));
}

#[tokio::test]
async fn corrupt_subject_load_captures_revision() {
    let kv = MockJetStreamKvStore::new();
    let record = record("orders");
    kv.enqueue_entry(Bytes::from_static(b"not proto"), 5, kv::Operation::Put);
    let store = ScheduleCheckpointStore::new(kv);

    let error = store.load_by_subject(&record.subject()).await.unwrap_err();

    assert_eq!(error.corrupt_revision(), Some(5));
}

#[tokio::test]
async fn backend_errors_have_no_corrupt_revision() {
    let kv = MockJetStreamKvStore::new();
    kv.enqueue_entry_error(kv::EntryErrorKind::Other);
    let store = ScheduleCheckpointStore::new(kv);
    let error = store
        .load(&ScheduleKey::derive(&ScheduleId::parse("orders").unwrap()))
        .await
        .unwrap_err();

    assert!(error.is_transient());
    assert!(error.corrupt_revision().is_none());
    assert!(error.corrupt_watermark().is_none());
}

#[tokio::test]
async fn list_skips_checkpoint_keys_without_values() {
    let kv = MockJetStreamKvStore::new();
    let record = record("orders");
    kv.set_keys_result(Ok(vec![format!("v1.{}", record.key().simple())]));
    kv.enqueue_get_none();
    let store = ScheduleCheckpointStore::new(kv);

    let records = store.list().await.unwrap();

    assert!(records.is_empty());
}
