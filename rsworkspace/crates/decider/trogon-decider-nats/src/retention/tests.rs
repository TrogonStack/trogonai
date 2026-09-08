use super::*;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use trogon_decider_runtime::snapshot::{
    InvalidSnapshotTypeNameError, SnapshotPayloadData, SnapshotTypeName, encode_snapshot,
};
use trogon_nats::jetstream::mocks::MockJetStreamKvStore;

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).expect("test stream position must be non-zero")
}

fn discard_below(value: u64) -> RetentionWatermark {
    RetentionWatermark::DiscardBelow(position(value))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TestPayload {
    id: String,
}

impl SnapshotType for TestPayload {
    type Error = InvalidSnapshotTypeNameError;

    fn snapshot_type() -> Result<SnapshotTypeName, Self::Error> {
        SnapshotTypeName::new("test.retention.v1")
    }
}

impl SnapshotPayloadDecode for TestPayload {
    type Error = serde_json::Error;

    fn decode(payload: SnapshotPayloadData<'_>) -> Result<Self, Self::Error> {
        serde_json::from_slice(payload.payload)
    }
}

impl trogon_decider_runtime::snapshot::SnapshotPayloadEncode for TestPayload {
    type Error = serde_json::Error;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(self)
    }
}

fn snapshots(entries: &[(&str, u64)]) -> BTreeMap<String, Snapshot<TestPayload>> {
    entries
        .iter()
        .map(|(snapshot_id, value)| {
            (
                (*snapshot_id).to_string(),
                Snapshot::new(
                    position(*value),
                    TestPayload {
                        id: (*snapshot_id).to_string(),
                    },
                ),
            )
        })
        .collect()
}

fn encoded_snapshot(snapshot_id: &str, value: u64) -> Bytes {
    let snapshot = Snapshot::new(
        position(value),
        TestPayload {
            id: snapshot_id.to_string(),
        },
    );
    Bytes::from(
        encode_snapshot(&snapshot)
            .expect("test snapshot must encode")
            .into_bytes()
            .expect("test snapshot envelope must encode"),
    )
}

#[test]
fn a_stream_nobody_snapshotted_retains_everything() {
    let watermarks = RetentionWatermarksBuilder::new()
        .observe_snapshots(&snapshots(&[("alpha", 7)]))
        .build();

    assert_eq!(watermarks.watermark_for("beta"), RetentionWatermark::RetainAll);
}

#[test]
fn one_snapshot_type_reports_each_snapshot_position() {
    let watermarks = RetentionWatermarksBuilder::new()
        .observe_snapshots(&snapshots(&[("alpha", 7), ("beta", 12)]))
        .build();

    assert_eq!(watermarks.watermark_for("alpha"), discard_below(7));
    assert_eq!(watermarks.watermark_for("beta"), discard_below(12));
    assert_eq!(watermarks.len(), 2);
}

#[test]
fn the_lowest_position_across_snapshot_types_wins() {
    let watermarks = RetentionWatermarksBuilder::new()
        .observe_snapshots(&snapshots(&[("alpha", 20)]))
        .observe_snapshots(&snapshots(&[("alpha", 9)]))
        .build();

    assert_eq!(watermarks.watermark_for("alpha"), discard_below(9));
}

#[test]
fn a_stream_missing_from_one_snapshot_type_retains_everything() {
    let watermarks = RetentionWatermarksBuilder::new()
        .observe_snapshots(&snapshots(&[("alpha", 20), ("beta", 30)]))
        .observe_snapshots(&snapshots(&[("alpha", 25)]))
        .build();

    assert_eq!(watermarks.watermark_for("alpha"), discard_below(20));
    assert_eq!(watermarks.watermark_for("beta"), RetentionWatermark::RetainAll);
}

#[test]
fn a_declared_stream_without_a_snapshot_retains_everything() {
    let watermarks = RetentionWatermarksBuilder::new()
        .observe_snapshots(&snapshots(&[("alpha", 20)]))
        .observe_stream_ids(["beta"])
        .build();

    assert_eq!(watermarks.watermark_for("beta"), RetentionWatermark::RetainAll);
    assert_eq!(watermarks.aggregate(), RetentionWatermark::RetainAll);
}

#[test]
fn a_checkpoint_bounds_every_stream() {
    let watermarks = RetentionWatermarksBuilder::new()
        .observe_snapshots(&snapshots(&[("alpha", 20), ("beta", 4)]))
        .observe_checkpoint(CheckpointSequence::new(11))
        .build();

    assert_eq!(watermarks.watermark_for("alpha"), discard_below(11));
    assert_eq!(watermarks.watermark_for("beta"), discard_below(4));
}

#[test]
fn the_lowest_checkpoint_bounds_every_stream() {
    let watermarks = RetentionWatermarksBuilder::new()
        .observe_snapshots(&snapshots(&[("alpha", 20)]))
        .observe_checkpoint(CheckpointSequence::new(11))
        .observe_checkpoint(CheckpointSequence::new(6))
        .build();

    assert_eq!(watermarks.watermark_for("alpha"), discard_below(6));
}

#[test]
fn a_checkpoint_with_no_progress_retains_everything() {
    let watermarks = RetentionWatermarksBuilder::new()
        .observe_snapshots(&snapshots(&[("alpha", 20)]))
        .observe_checkpoint(CheckpointSequence::NONE)
        .build();

    assert_eq!(watermarks.watermark_for("alpha"), RetentionWatermark::RetainAll);
}

#[test]
fn the_aggregate_is_the_lowest_watermark_across_streams() {
    let watermarks = RetentionWatermarksBuilder::new()
        .observe_snapshots(&snapshots(&[("alpha", 20), ("beta", 4), ("gamma", 9)]))
        .build();

    assert_eq!(watermarks.aggregate(), discard_below(4));
    assert_eq!(
        watermarks.streams().collect::<Vec<_>>(),
        vec![
            ("alpha", discard_below(20)),
            ("beta", discard_below(4)),
            ("gamma", discard_below(9)),
        ]
    );
}

#[test]
fn a_report_that_knows_nothing_retains_everything() {
    let watermarks = RetentionWatermarksBuilder::new().build();

    assert!(watermarks.is_empty());
    assert_eq!(watermarks.aggregate(), RetentionWatermark::RetainAll);
    assert_eq!(watermarks.watermark_for("alpha"), RetentionWatermark::RetainAll);
}

#[test]
fn retain_all_is_the_most_conservative_watermark() {
    assert_eq!(
        RetentionWatermark::RetainAll.min(discard_below(3)),
        RetentionWatermark::RetainAll
    );
    assert_eq!(discard_below(3).min(discard_below(9)), discard_below(3));
    assert_eq!(RetentionWatermark::RetainAll.lowest_retained_sequence(), None);
    assert_eq!(discard_below(3).lowest_retained_sequence(), Some(position(3)));
    assert!(RetentionWatermark::RetainAll.retains_all());
    assert!(!discard_below(3).retains_all());
}

#[test]
fn a_watermark_renders_its_boundary() {
    assert_eq!(RetentionWatermark::RetainAll.to_string(), "retain-all");
    assert_eq!(discard_below(42).to_string(), "discard-below-42");
}

#[tokio::test]
async fn reading_watermarks_folds_the_snapshots_and_the_checkpoint() {
    let bucket = MockJetStreamKvStore::new();
    bucket.set_keys_result(Ok(vec![
        "snapshots.data.test.retention.v1.alpha".to_string(),
        "snapshots.data.test.retention.v1.beta".to_string(),
    ]));
    bucket.enqueue_get_some(encoded_snapshot("alpha", 20));
    bucket.enqueue_get_some(encoded_snapshot("beta", 4));
    bucket.enqueue_entry(Bytes::from("11"), 3, async_nats::jetstream::kv::Operation::Put);

    let watermarks = read_retention_watermarks::<TestPayload, _>(
        &bucket,
        &NatsSnapshotConfig::with_checkpoint_name("last_event_sequence"),
    )
    .await
    .expect("watermarks should be readable");

    assert_eq!(watermarks.watermark_for("alpha"), discard_below(11));
    assert_eq!(watermarks.watermark_for("beta"), discard_below(4));
    assert_eq!(watermarks.aggregate(), discard_below(4));
    assert_eq!(
        bucket.entry_calls(),
        vec!["snapshots.checkpoint.test.retention.v1.last_event_sequence".to_string()]
    );
}

#[tokio::test]
async fn reading_watermarks_skips_an_unconfigured_checkpoint() {
    let bucket = MockJetStreamKvStore::new();
    bucket.set_keys_result(Ok(vec!["snapshots.data.test.retention.v1.alpha".to_string()]));
    bucket.enqueue_get_some(encoded_snapshot("alpha", 20));

    let watermarks = read_retention_watermarks::<TestPayload, _>(&bucket, &NatsSnapshotConfig::without_checkpoint())
        .await
        .expect("watermarks should be readable");

    assert_eq!(watermarks.watermark_for("alpha"), discard_below(20));
    assert!(bucket.entry_calls().is_empty());
}
