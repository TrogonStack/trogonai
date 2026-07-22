use super::*;
use crate::SnapshotTypeName;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TestPayload {
    id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct UnavailableSnapshotTypePayload {
    id: String,
}

#[derive(Debug, thiserror::Error)]
#[error("missing snapshot type")]
struct TestSnapshotTypeError;

impl SnapshotPayloadEncode for TestPayload {
    type Error = serde_json::Error;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(self)
    }
}

impl SnapshotPayloadDecode for TestPayload {
    type Error = serde_json::Error;

    fn decode(payload: SnapshotPayloadData<'_>) -> Result<Self, Self::Error> {
        serde_json::from_slice(payload.payload)
    }
}

impl SnapshotType for TestPayload {
    type Error = crate::InvalidSnapshotTypeNameError;

    fn snapshot_type() -> Result<SnapshotTypeName, Self::Error> {
        SnapshotTypeName::new("test.snapshot.v1")
    }
}

impl SnapshotPayloadEncode for UnavailableSnapshotTypePayload {
    type Error = serde_json::Error;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(self)
    }
}

impl SnapshotPayloadDecode for UnavailableSnapshotTypePayload {
    type Error = serde_json::Error;

    fn decode(payload: SnapshotPayloadData<'_>) -> Result<Self, Self::Error> {
        serde_json::from_slice(payload.payload)
    }
}

impl SnapshotType for UnavailableSnapshotTypePayload {
    type Error = TestSnapshotTypeError;

    fn snapshot_type() -> Result<SnapshotTypeName, Self::Error> {
        Err(TestSnapshotTypeError)
    }
}

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).expect("test stream position must be non-zero")
}

#[test]
fn snapshot_round_trips_through_encoded_snapshot() {
    let snapshot = Snapshot::new(
        position(7),
        TestPayload {
            id: "backup".to_string(),
        },
    );

    let encoded = encode_snapshot(&snapshot).unwrap();
    let decoded = decode_snapshot::<TestPayload>(encoded).unwrap();

    assert_eq!(decoded, snapshot);
}

#[test]
fn encoded_snapshot_round_trips_through_envelope_bytes() {
    let encoded = EncodedSnapshot::new("test.snapshot.v1", position(7), br#"{"id":"backup"}"#.to_vec());
    let expected = encoded.clone();

    let json = String::from_utf8(encoded.into_bytes().unwrap()).unwrap();
    let decoded = EncodedSnapshot::from_bytes(json.as_bytes()).unwrap();

    assert!(json.contains("\"type\":\"test.snapshot.v1\""));
    assert!(json.contains("\"position\":7"));
    assert!(json.contains("\"payload\""));
    assert_eq!(decoded, expected);
}

#[test]
fn decode_snapshot_rejects_unexpected_type() {
    let encoded = EncodedSnapshot::new("other.snapshot.v1", position(7), br#"{"id":"backup"}"#.to_vec());

    let error = decode_snapshot::<TestPayload>(encoded).unwrap_err();

    assert_eq!(
        error.to_string(),
        "unexpected snapshot type: expected test.snapshot.v1, got other.snapshot.v1"
    );
}

#[test]
fn encode_snapshot_preserves_snapshot_type_error() {
    let snapshot = Snapshot::new(
        position(7),
        UnavailableSnapshotTypePayload {
            id: "backup".to_string(),
        },
    );

    let error = encode_snapshot(&snapshot).unwrap_err();

    assert_eq!(
        error.to_string(),
        "failed to resolve snapshot type: missing snapshot type"
    );
    assert!(error.snapshot_type_source().is_some());
}

#[test]
fn decode_snapshot_preserves_snapshot_type_error() {
    let encoded = EncodedSnapshot::new("test.snapshot.v1", position(7), br#"{"id":"backup"}"#.to_vec());

    let error = decode_snapshot::<UnavailableSnapshotTypePayload>(encoded).unwrap_err();

    assert_eq!(
        error.to_string(),
        "failed to resolve snapshot type: missing snapshot type"
    );
    assert!(error.snapshot_type_source().is_some());
}

#[test]
fn unavailable_snapshot_type_payload_codecs_round_trip() {
    let payload = UnavailableSnapshotTypePayload {
        id: "backup".to_string(),
    };
    let encoded = payload.encode().unwrap();
    let decoded = UnavailableSnapshotTypePayload::decode(SnapshotPayloadData::new(encoded.as_slice())).unwrap();

    assert_eq!(decoded, payload);
}
