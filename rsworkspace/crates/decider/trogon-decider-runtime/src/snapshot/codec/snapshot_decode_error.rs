use crate::SnapshotTypeName;

/// Error returned by [`decode_snapshot`](super::decode_snapshot) when a
/// snapshot cannot be turned back into a typed decider state.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotDecodeError<PayloadSource, SnapshotTypeSource = std::convert::Infallible> {
    /// The decider state's own snapshot type name could not be resolved.
    #[error("failed to resolve snapshot type: {source}")]
    SnapshotType {
        /// The underlying [`SnapshotType`](crate::SnapshotType) failure.
        #[source]
        source: SnapshotTypeSource,
    },
    /// The encoded payload bytes could not be decoded into the decider state.
    #[error("failed to decode snapshot payload: {source}")]
    Payload {
        /// The underlying [`SnapshotPayloadDecode`](super::SnapshotPayloadDecode) failure.
        #[source]
        source: PayloadSource,
    },
    /// The snapshot's stored type name does not match the decider state's
    /// expected type name, so decoding was refused before touching the payload.
    #[error("unexpected snapshot type: expected {expected}, got {actual}")]
    UnexpectedType {
        /// Type name the decider state expected.
        expected: SnapshotTypeName,
        /// Type name actually stored on the snapshot.
        actual: String,
    },
}

impl<PayloadSource, SnapshotTypeSource> SnapshotDecodeError<PayloadSource, SnapshotTypeSource> {
    pub(super) fn snapshot_type(source: SnapshotTypeSource) -> Self {
        Self::SnapshotType { source }
    }

    pub(super) fn payload(source: PayloadSource) -> Self {
        Self::Payload { source }
    }

    pub(super) fn unexpected_type(expected: SnapshotTypeName, actual: String) -> Self {
        Self::UnexpectedType { expected, actual }
    }

    /// Returns the payload decode failure, if that was the cause.
    pub fn payload_source(&self) -> Option<&PayloadSource> {
        match self {
            Self::Payload { source } => Some(source),
            Self::SnapshotType { .. } | Self::UnexpectedType { .. } => None,
        }
    }

    /// Returns the snapshot type resolution failure, if that was the cause.
    pub fn snapshot_type_source(&self) -> Option<&SnapshotTypeSource> {
        match self {
            Self::SnapshotType { source } => Some(source),
            Self::Payload { .. } | Self::UnexpectedType { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests;
