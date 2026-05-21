use serde::{Deserialize, Serialize};

use crate::{Snapshot, StreamPosition};

use super::{
    SnapshotDecodeError, SnapshotEncodeError, SnapshotEnvelopeDecodeError, SnapshotEnvelopeEncodeError,
    SnapshotPayloadData, SnapshotPayloadDecode, SnapshotPayloadEncode,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedSnapshot {
    pub position: StreamPosition,
    pub payload: Vec<u8>,
}

impl EncodedSnapshot {
    pub fn new(position: StreamPosition, payload: Vec<u8>) -> Self {
        Self { position, payload }
    }

    pub fn into_bytes(self) -> Result<Vec<u8>, SnapshotEnvelopeEncodeError> {
        serde_json::to_vec(&SnapshotEnvelope {
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

        Ok(Self::new(position, envelope.payload))
    }
}

pub fn encode_snapshot<T>(snapshot: &Snapshot<T>) -> Result<EncodedSnapshot, SnapshotEncodeError>
where
    T: SnapshotPayloadEncode,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    let payload = snapshot.payload.encode().map_err(SnapshotEncodeError::new)?;

    Ok(EncodedSnapshot::new(snapshot.position, payload))
}

pub fn decode_snapshot<T>(encoded: EncodedSnapshot) -> Result<Snapshot<T>, SnapshotDecodeError>
where
    T: SnapshotPayloadDecode,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    let payload = T::decode(SnapshotPayloadData::new(encoded.payload.as_slice())).map_err(SnapshotDecodeError::new)?;

    Ok(Snapshot::new(encoded.position, payload))
}

#[derive(Debug, Serialize, Deserialize)]
struct SnapshotEnvelope {
    position: u64,
    payload: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    struct TestPayload {
        id: String,
    }

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
        let encoded = EncodedSnapshot::new(position(7), br#"{"id":"backup"}"#.to_vec());
        let expected = encoded.clone();

        let json = String::from_utf8(encoded.into_bytes().unwrap()).unwrap();
        let decoded = EncodedSnapshot::from_bytes(json.as_bytes()).unwrap();

        assert!(json.contains("\"position\":7"));
        assert!(json.contains("\"payload\""));
        assert_eq!(decoded, expected);
    }
}
