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
mod tests;
