use super::*;
use bytes::Bytes;
use futures::stream::{self, BoxStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use trogon_nats::jetstream::mocks::MockJetStreamKvStore;

fn stored_snapshot(sequence: u64) -> Bytes {
    snapshot_value(&Snapshot::new(position(sequence), TestPayload { id: "alpha".into() }))
        .unwrap()
        .into()
}

struct UnencodablePayload;

impl SnapshotType for UnencodablePayload {
    type Error = InvalidSnapshotTypeNameError;

    fn snapshot_type() -> Result<SnapshotTypeName, Self::Error> {
        TestPayload::snapshot_type()
    }
}

impl SnapshotPayloadEncode for UnencodablePayload {
    type Error = std::io::Error;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "payload rejected"))
    }
}

impl SnapshotPayloadEncode for UnavailableSnapshotTypePayload {
    type Error = std::io::Error;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        Err(std::io::Error::other("type resolution must precede payload encoding"))
    }
}

#[tokio::test]
async fn snapshot_encoding_failure_preserves_cause_and_does_not_touch_storage() {
    let bucket = MockJetStreamKvStore::new();
    assert!(matches!(
        write_snapshot(&bucket, "alpha", Snapshot::new(position(1), UnencodablePayload)).await,
        Err(SnapshotStoreError::Codec(SnapshotCodecError::EncodePayload { source }))
            if source.kind() == std::io::ErrorKind::InvalidData
    ));
    assert!(bucket.entry_calls().is_empty());
    assert!(bucket.create_calls().is_empty());
    assert!(bucket.update_calls().is_empty());

    assert!(matches!(
        write_snapshot(
            &bucket,
            "alpha",
            Snapshot::new(position(1), UnavailableSnapshotTypePayload)
        )
        .await,
        Err(SnapshotStoreError::Codec(SnapshotCodecError::SnapshotType {
            source: TestSnapshotTypeError
        }))
    ));
    assert!(bucket.entry_calls().is_empty());
    assert!(bucket.create_calls().is_empty());
    assert!(bucket.update_calls().is_empty());
}

#[tokio::test]
async fn read_failures_preserve_the_failed_storage_operation() {
    let bucket = MockJetStreamKvStore::new();
    bucket.enqueue_get_error(kv::EntryErrorKind::TimedOut);
    assert!(matches!(
        read_snapshot::<TestPayload, _>(&bucket, "alpha").await,
        Err(SnapshotStoreError::Kv(SnapshotKvError::ReadSnapshotEntry { source }))
            if source.kind() == kv::EntryErrorKind::TimedOut
    ));
    assert_eq!(bucket.get_calls(), ["snapshots.data.test.snapshot.v2.alpha"]);

    let bucket = MockJetStreamKvStore::new();
    bucket.set_keys_result(Err(kv::WatchErrorKind::ConsumerCreate));
    assert!(matches!(
        list_snapshots::<TestPayload, _>(&bucket).await,
        Err(SnapshotStoreError::Kv(SnapshotKvError::ListSnapshotKeys { source }))
            if source.kind() == kv::WatchErrorKind::ConsumerCreate
    ));
    assert_eq!(bucket.keys_calls(), 1);
    assert!(bucket.get_calls().is_empty());

    let bucket = MockJetStreamKvStore::new();
    bucket.set_keys_result(Ok(vec!["snapshots.data.test.snapshot.v2.alpha".into()]));
    bucket.enqueue_get_error(kv::EntryErrorKind::TimedOut);
    assert!(matches!(
        read_snapshot_map::<TestPayload, _>(&bucket).await,
        Err(SnapshotStoreError::Kv(SnapshotKvError::ReadSnapshotValue { source }))
            if source.kind() == kv::EntryErrorKind::TimedOut
    ));
    assert_eq!(bucket.get_calls(), ["snapshots.data.test.snapshot.v2.alpha"]);
}

#[derive(Clone, Default)]
struct FailedKeyStream {
    get_calls: Arc<AtomicUsize>,
}

impl JetStreamKvGet for FailedKeyStream {
    async fn get(&self, _key: String) -> Result<Option<Bytes>, kv::EntryError> {
        self.get_calls.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
}

impl JetStreamKvKeys for FailedKeyStream {
    type Keys = BoxStream<'static, Result<String, kv::WatcherError>>;

    async fn keys(&self) -> Result<Self::Keys, kv::HistoryError> {
        Ok(Box::pin(stream::iter([Err(kv::WatcherError::new(
            kv::WatcherErrorKind::Consumer,
        ))])))
    }
}

#[tokio::test]
async fn interrupted_key_stream_returns_failure_without_fetching_an_unknown_key() {
    let bucket = FailedKeyStream::default();
    assert!(matches!(
        list_snapshots::<TestPayload, _>(&bucket).await,
        Err(SnapshotStoreError::Kv(SnapshotKvError::ReadSnapshotKey { source }))
            if source.kind() == kv::WatcherErrorKind::Consumer
    ));
    assert_eq!(bucket.get_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn malformed_snapshot_prevents_returning_a_partial_listing() {
    let bucket = MockJetStreamKvStore::new();
    bucket.set_keys_result(Ok(vec![
        "snapshots.data.test.snapshot.v2.alpha".into(),
        "snapshots.data.test.snapshot.v2.beta".into(),
    ]));
    bucket.enqueue_get_some(stored_snapshot(3));
    bucket.enqueue_get_some(Bytes::from_static(b"invalid snapshot envelope"));

    assert!(matches!(
        list_snapshots::<TestPayload, _>(&bucket).await,
        Err(SnapshotStoreError::Codec(SnapshotCodecError::DecodeEnvelope { .. }))
    ));
    assert_eq!(bucket.get_calls().len(), 2);
}

#[tokio::test]
async fn checkpoint_read_failure_and_corruption_do_not_advance_the_checkpoint() {
    let config = NatsSnapshotConfig::with_checkpoint_name("worker");
    let bucket = MockJetStreamKvStore::new();
    bucket.enqueue_entry_error(kv::EntryErrorKind::TimedOut);
    assert!(matches!(
        maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 1).await,
        Err(SnapshotStoreError::Kv(SnapshotKvError::ReadCheckpointEntry { source }))
            if source.kind() == kv::EntryErrorKind::TimedOut
    ));
    assert!(bucket.create_calls().is_empty());
    assert!(bucket.update_calls().is_empty());

    for invalid in [b"\xff".as_slice(), b"-1", b"18446744073709551616", b""] {
        let bucket = MockJetStreamKvStore::new();
        bucket.enqueue_entry(Bytes::copy_from_slice(invalid), 7, kv::Operation::Put);
        assert!(matches!(
            maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 1).await,
            Err(SnapshotStoreError::Kv(SnapshotKvError::DecodeCheckpoint { key }))
                if key == "snapshots.checkpoint.test.snapshot.v2.worker"
        ));
        assert!(bucket.create_calls().is_empty());
        assert!(bucket.update_calls().is_empty());
    }
}

#[tokio::test]
async fn checkpoint_advance_propagates_storage_failures_without_treating_them_as_conflicts() {
    let config = NatsSnapshotConfig::with_checkpoint_name("worker");
    let bucket = MockJetStreamKvStore::new();
    bucket.enqueue_entry(Bytes::from_static(b"4"), 8, kv::Operation::Put);
    bucket.enqueue_update_result(Err(kv::UpdateErrorKind::TimedOut));
    assert!(matches!(
        maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 5).await,
        Err(SnapshotStoreError::Kv(SnapshotKvError::AdvanceCheckpoint { source }))
            if source.kind() == kv::UpdateErrorKind::TimedOut
    ));
    assert_eq!(
        bucket.update_calls(),
        [(
            "snapshots.checkpoint.test.snapshot.v2.worker".into(),
            Bytes::from_static(b"5"),
            8
        )]
    );
    assert_eq!(bucket.entry_calls().len(), 1);

    let bucket = MockJetStreamKvStore::new();
    bucket.enqueue_entry_none();
    bucket.enqueue_create_result(Err(kv::CreateErrorKind::Publish));
    assert!(matches!(
        maybe_advance_checkpoint::<TestPayload, _>(&bucket, &config, 1).await,
        Err(SnapshotStoreError::Kv(SnapshotKvError::CreateCheckpoint { source }))
            if source.kind() == kv::CreateErrorKind::Publish
    ));
    assert_eq!(
        bucket.create_calls(),
        [(
            "snapshots.checkpoint.test.snapshot.v2.worker".into(),
            Bytes::from_static(b"1")
        )]
    );
    assert_eq!(bucket.entry_calls().len(), 1);
}

#[tokio::test]
async fn checkpoint_write_reloads_revision_after_create_and_update_conflicts() {
    let bucket = MockJetStreamKvStore::new();
    let config = NatsSnapshotConfig::with_checkpoint_name("worker");
    bucket.enqueue_entry_none();
    bucket.enqueue_create_result(Err(kv::CreateErrorKind::AlreadyExists));
    bucket.enqueue_entry(Bytes::from_static(b"4"), 3, kv::Operation::Put);
    bucket.enqueue_update_result(Err(kv::UpdateErrorKind::WrongLastRevision));
    bucket.enqueue_entry(Bytes::from_static(b"5"), 9, kv::Operation::Put);
    bucket.enqueue_update_result(Ok(10));

    write_checkpoint::<TestPayload, _>(&bucket, &config, 20).await.unwrap();

    assert_eq!(
        bucket.entry_calls(),
        vec!["snapshots.checkpoint.test.snapshot.v2.worker"; 3]
    );
    assert_eq!(
        bucket.create_calls(),
        [(
            "snapshots.checkpoint.test.snapshot.v2.worker".into(),
            Bytes::from_static(b"20")
        )]
    );
    assert_eq!(
        bucket.update_calls(),
        [
            (
                "snapshots.checkpoint.test.snapshot.v2.worker".into(),
                Bytes::from_static(b"20"),
                3
            ),
            (
                "snapshots.checkpoint.test.snapshot.v2.worker".into(),
                Bytes::from_static(b"20"),
                9
            ),
        ]
    );
}

#[tokio::test]
async fn checkpoint_write_preserves_read_create_and_update_errors() {
    let config = NatsSnapshotConfig::with_checkpoint_name("worker");
    let bucket = MockJetStreamKvStore::new();
    bucket.enqueue_entry_error(kv::EntryErrorKind::TimedOut);
    assert!(matches!(
        write_checkpoint::<TestPayload, _>(&bucket, &config, 5).await,
        Err(SnapshotStoreError::Kv(SnapshotKvError::ReadEntryForUpdate { source }))
            if source.kind() == kv::EntryErrorKind::TimedOut
    ));
    assert!(bucket.create_calls().is_empty());
    assert!(bucket.update_calls().is_empty());

    let bucket = MockJetStreamKvStore::new();
    bucket.enqueue_entry_none();
    bucket.enqueue_create_result(Err(kv::CreateErrorKind::Publish));
    assert!(matches!(
        write_checkpoint::<TestPayload, _>(&bucket, &config, 5).await,
        Err(SnapshotStoreError::Kv(SnapshotKvError::CreateEntry { source }))
            if source.kind() == kv::CreateErrorKind::Publish
    ));
    assert_eq!(bucket.entry_calls().len(), 1);
    assert_eq!(bucket.create_calls().len(), 1);

    let bucket = MockJetStreamKvStore::new();
    bucket.enqueue_entry(Bytes::from_static(b"3"), 4, kv::Operation::Put);
    bucket.enqueue_update_result(Err(kv::UpdateErrorKind::TimedOut));
    assert!(matches!(
        write_checkpoint::<TestPayload, _>(&bucket, &config, 5).await,
        Err(SnapshotStoreError::Kv(SnapshotKvError::UpdateEntry { source }))
            if source.kind() == kv::UpdateErrorKind::TimedOut
    ));
    assert_eq!(bucket.entry_calls().len(), 1);
    assert_eq!(bucket.update_calls().len(), 1);
}

#[tokio::test]
async fn snapshot_write_preserves_read_create_and_update_errors() {
    let snapshot = Snapshot::new(position(9), TestPayload { id: "alpha".into() });
    let bucket = MockJetStreamKvStore::new();
    bucket.enqueue_entry_error(kv::EntryErrorKind::TimedOut);
    assert!(matches!(
        write_snapshot(&bucket, "alpha", snapshot.clone()).await,
        Err(SnapshotStoreError::Kv(SnapshotKvError::ReadEntryForSnapshotUpdate { source }))
            if source.kind() == kv::EntryErrorKind::TimedOut
    ));
    assert!(bucket.create_calls().is_empty());
    assert!(bucket.update_calls().is_empty());

    let bucket = MockJetStreamKvStore::new();
    bucket.enqueue_entry_none();
    bucket.enqueue_create_result(Err(kv::CreateErrorKind::Publish));
    assert!(matches!(
        write_snapshot(&bucket, "alpha", snapshot.clone()).await,
        Err(SnapshotStoreError::Kv(SnapshotKvError::CreateSnapshotEntry { source }))
            if source.kind() == kv::CreateErrorKind::Publish
    ));
    assert_eq!(bucket.entry_calls().len(), 1);
    assert_eq!(bucket.create_calls().len(), 1);

    let bucket = MockJetStreamKvStore::new();
    bucket.enqueue_entry(stored_snapshot(3), 4, kv::Operation::Put);
    bucket.enqueue_update_result(Err(kv::UpdateErrorKind::TimedOut));
    assert!(matches!(
        write_snapshot(&bucket, "alpha", snapshot).await,
        Err(SnapshotStoreError::Kv(SnapshotKvError::UpdateSnapshotEntry { source }))
            if source.kind() == kv::UpdateErrorKind::TimedOut
    ));
    assert_eq!(bucket.entry_calls().len(), 1);
    assert_eq!(bucket.update_calls().len(), 1);
}

#[tokio::test]
async fn snapshot_write_reloads_after_tombstone_recreation_race() {
    let bucket = MockJetStreamKvStore::new();
    bucket.enqueue_entry(Bytes::new(), 4, kv::Operation::Purge);
    bucket.enqueue_create_result(Err(kv::CreateErrorKind::AlreadyExists));
    bucket.enqueue_entry(stored_snapshot(12), 7, kv::Operation::Put);

    write_snapshot(
        &bucket,
        "alpha",
        Snapshot::new(position(9), TestPayload { id: "older".into() }),
    )
    .await
    .unwrap();

    assert_eq!(bucket.entry_calls(), vec!["snapshots.data.test.snapshot.v2.alpha"; 2]);
    assert_eq!(bucket.create_calls().len(), 1);
    assert!(bucket.update_calls().is_empty());
}

#[tokio::test]
async fn unreadable_existing_snapshot_is_not_overwritten() {
    let bucket = MockJetStreamKvStore::new();
    bucket.enqueue_entry(Bytes::from_static(b"broken envelope"), 4, kv::Operation::Put);
    assert!(matches!(
        write_snapshot(
            &bucket,
            "alpha",
            Snapshot::new(position(9), TestPayload { id: "alpha".into() })
        )
        .await,
        Err(SnapshotStoreError::Codec(SnapshotCodecError::DecodeEnvelope { .. }))
    ));
    assert!(bucket.create_calls().is_empty());
    assert!(bucket.update_calls().is_empty());
}

#[tokio::test]
async fn snapshot_delete_propagates_read_and_delete_failures_without_retrying() {
    let bucket = MockJetStreamKvStore::new();
    bucket.enqueue_entry_error(kv::EntryErrorKind::TimedOut);
    assert!(matches!(
        persist_snapshot_change::<TestPayload, _>(&bucket, SnapshotChange::delete("alpha")).await,
        Err(SnapshotStoreError::Kv(SnapshotKvError::ReadEntryForDelete { source }))
            if source.kind() == kv::EntryErrorKind::TimedOut
    ));
    assert!(bucket.delete_calls().is_empty());

    let bucket = MockJetStreamKvStore::new();
    bucket.enqueue_entry(stored_snapshot(3), 4, kv::Operation::Put);
    bucket.enqueue_delete_result(Err(kv::DeleteErrorKind::TimedOut));
    assert!(matches!(
        persist_snapshot_change::<TestPayload, _>(&bucket, SnapshotChange::delete("alpha")).await,
        Err(SnapshotStoreError::Kv(SnapshotKvError::DeleteEntry { source }))
            if source.kind() == kv::DeleteErrorKind::TimedOut
    ));
    assert_eq!(bucket.entry_calls().len(), 1);
    assert_eq!(
        bucket.delete_calls(),
        [("snapshots.data.test.snapshot.v2.alpha".into(), Some(4))]
    );
}
