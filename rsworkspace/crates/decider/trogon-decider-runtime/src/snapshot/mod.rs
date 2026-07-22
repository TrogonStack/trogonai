mod codec;
mod read_snapshot;
mod snapshot_type;
mod write_snapshot;

use crate::StreamPosition;

pub use codec::{
    EncodedSnapshot, SnapshotDecodeError, SnapshotEncodeError, SnapshotEnvelopeDecodeError,
    SnapshotEnvelopeEncodeError, SnapshotPayloadData, SnapshotPayloadDecode, SnapshotPayloadEncode, decode_snapshot,
    encode_snapshot,
};
pub use read_snapshot::{ReadSnapshotRequest, ReadSnapshotResponse, SnapshotRead};
pub use snapshot_type::{InvalidSnapshotTypeNameError, SnapshotType, SnapshotTypeName};
pub use write_snapshot::{SnapshotWrite, WriteSnapshotRequest, WriteSnapshotResponse};

/// A point-in-time capture of decider state, tagged with the stream position
/// it was taken at so replay can resume strictly after it.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot<T> {
    /// The stream position covered by the snapshot payload.
    ///
    /// This is a replay/checkpoint boundary. It is intentionally a
    /// `StreamPosition`, not a revision, because adapters are allowed to use
    /// sparse but comparable positions.
    pub position: StreamPosition,
    /// The captured decider state.
    pub payload: T,
}

impl<T> Snapshot<T> {
    /// Pairs a decider state with the stream position it was captured at.
    pub fn new(position: StreamPosition, payload: T) -> Self {
        Self { position, payload }
    }
}
