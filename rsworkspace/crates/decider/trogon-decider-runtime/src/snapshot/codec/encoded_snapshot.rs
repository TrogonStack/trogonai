use serde::{Deserialize, Serialize};

use crate::{Snapshot, SnapshotType, StreamPosition};

use super::{
    SnapshotDecodeError, SnapshotEncodeError, SnapshotEnvelopeDecodeError, SnapshotEnvelopeEncodeError,
    SnapshotPayloadData, SnapshotPayloadDecode, SnapshotPayloadEncode,
};

/// A snapshot payload after it has been encoded to bytes but before it is
/// serialized to the wire envelope, or after the envelope has been parsed but
/// before the payload bytes are decoded back into a decider state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedSnapshot {
    /// Stable snapshot type name, used to reject a payload encoded for a
    /// different decider state.
    pub r#type: String,
    /// The stream position covered by the snapshot payload.
    pub position: StreamPosition,
    /// The encoded decider state bytes.
    pub payload: Vec<u8>,
}

impl EncodedSnapshot {
    /// Assembles an encoded snapshot from its type name, position, and
    /// already-encoded payload bytes.
    pub fn new(snapshot_type: impl Into<String>, position: StreamPosition, payload: Vec<u8>) -> Self {
        Self {
            r#type: snapshot_type.into(),
            position,
            payload,
        }
    }

    /// Serializes this snapshot into its on-the-wire envelope bytes.
    pub fn into_bytes(self) -> Result<Vec<u8>, SnapshotEnvelopeEncodeError> {
        serde_json::to_vec(&SnapshotEnvelope {
            r#type: self.r#type,
            position: self.position.as_u64(),
            payload: self.payload,
        })
        .map_err(SnapshotEnvelopeEncodeError::new)
    }

    /// Parses an on-the-wire envelope back into an encoded snapshot.
    pub fn from_bytes(value: &[u8]) -> Result<Self, SnapshotEnvelopeDecodeError> {
        let envelope =
            serde_json::from_slice::<SnapshotEnvelope>(value).map_err(SnapshotEnvelopeDecodeError::envelope_source)?;
        let position =
            StreamPosition::try_new(envelope.position).map_err(SnapshotEnvelopeDecodeError::position_source)?;

        Ok(Self::new(envelope.r#type, position, envelope.payload))
    }
}

/// Encodes a decider state snapshot's payload and resolves its type name,
/// producing a storage-neutral [`EncodedSnapshot`].
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

/// Decodes an [`EncodedSnapshot`] back into a typed decider state snapshot,
/// rejecting the payload if its stored type name does not match `T`'s.
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
