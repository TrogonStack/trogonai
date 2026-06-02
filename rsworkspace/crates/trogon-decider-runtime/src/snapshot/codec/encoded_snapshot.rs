use serde::{Deserialize, Serialize};

use crate::{Snapshot, SnapshotType, StreamPosition};

use super::{
    SnapshotDecodeError, SnapshotEncodeError, SnapshotEnvelopeDecodeError, SnapshotEnvelopeEncodeError,
    SnapshotPayloadData, SnapshotPayloadDecode, SnapshotPayloadEncode,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedSnapshot {
    pub r#type: String,
    pub position: StreamPosition,
    pub payload: Vec<u8>,
}

impl EncodedSnapshot {
    pub fn new(snapshot_type: impl Into<String>, position: StreamPosition, payload: Vec<u8>) -> Self {
        Self {
            r#type: snapshot_type.into(),
            position,
            payload,
        }
    }

    pub fn into_bytes(self) -> Result<Vec<u8>, SnapshotEnvelopeEncodeError> {
        serde_json::to_vec(&SnapshotEnvelope {
            r#type: self.r#type,
            position: self.position.as_u64(),
            payload: self.payload,
        })
        .map_err(SnapshotEnvelopeEncodeError::new)
    }

    pub fn from_bytes(value: &[u8]) -> Result<Self, SnapshotEnvelopeDecodeError> {
        let envelope =
            serde_json::from_slice::<SnapshotEnvelope>(value).map_err(SnapshotEnvelopeDecodeError::envelope_source)?;
        let position =
            StreamPosition::try_new(envelope.position).map_err(SnapshotEnvelopeDecodeError::position_source)?;

        Ok(Self::new(envelope.r#type, position, envelope.payload))
    }
}

pub fn encode_snapshot<T>(
    snapshot: &Snapshot<T>,
) -> Result<EncodedSnapshot, SnapshotEncodeError<<T as SnapshotPayloadEncode>::Error, <T as SnapshotType>::Error>>
where
    T: SnapshotPayloadEncode + SnapshotType,
{
    let snapshot_type = T::snapshot_type().map_err(SnapshotEncodeError::snapshot_type)?;
    let payload = snapshot.payload.encode().map_err(SnapshotEncodeError::payload)?;

    Ok(EncodedSnapshot::new(snapshot_type.as_str(), snapshot.position, payload))
}

pub fn decode_snapshot<T>(
    encoded: EncodedSnapshot,
) -> Result<Snapshot<T>, SnapshotDecodeError<<T as SnapshotPayloadDecode>::Error, <T as SnapshotType>::Error>>
where
    T: SnapshotPayloadDecode + SnapshotType,
{
    let snapshot_type = T::snapshot_type().map_err(SnapshotDecodeError::snapshot_type)?;
    if encoded.r#type != snapshot_type.as_str() {
        return Err(SnapshotDecodeError::unexpected_type(snapshot_type, encoded.r#type));
    }

    let payload =
        T::decode(SnapshotPayloadData::new(encoded.payload.as_slice())).map_err(SnapshotDecodeError::payload)?;

    Ok(Snapshot::new(encoded.position, payload))
}

#[derive(Debug, Serialize, Deserialize)]
struct SnapshotEnvelope {
    #[serde(rename = "type")]
    r#type: String,
    position: u64,
    payload: Vec<u8>,
}

#[cfg(test)]
mod tests {
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

    #[derive(Debug)]
    struct TestSnapshotTypeError;

    impl std::fmt::Display for TestSnapshotTypeError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("missing snapshot type")
        }
    }

    impl std::error::Error for TestSnapshotTypeError {}

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
        type Error = crate::InvalidSnapshotTypeName;

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
}
