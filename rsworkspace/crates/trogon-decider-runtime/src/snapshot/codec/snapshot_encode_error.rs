/// Error returned by [`encode_snapshot`](super::encode_snapshot) when a
/// decider state cannot be turned into an [`EncodedSnapshot`](super::EncodedSnapshot).
#[derive(Debug, thiserror::Error)]
pub enum SnapshotEncodeError<PayloadSource, SnapshotTypeSource = std::convert::Infallible> {
    /// The decider state's own snapshot type name could not be resolved.
    #[error("failed to resolve snapshot type: {source}")]
    SnapshotType {
        /// The underlying [`SnapshotType`](crate::SnapshotType) failure.
        #[source]
        source: SnapshotTypeSource,
    },
    /// The decider state could not be encoded into payload bytes.
    #[error("failed to encode snapshot payload: {source}")]
    Payload {
        /// The underlying [`SnapshotPayloadEncode`](super::SnapshotPayloadEncode) failure.
        #[source]
        source: PayloadSource,
    },
}

impl<PayloadSource, SnapshotTypeSource> SnapshotEncodeError<PayloadSource, SnapshotTypeSource> {
    pub(super) fn snapshot_type(source: SnapshotTypeSource) -> Self {
        Self::SnapshotType { source }
    }

    pub(super) fn payload(source: PayloadSource) -> Self {
        Self::Payload { source }
    }

    /// Returns the payload encode failure, if that was the cause.
    pub fn payload_source(&self) -> Option<&PayloadSource> {
        match self {
            Self::Payload { source } => Some(source),
            Self::SnapshotType { .. } => None,
        }
    }

    /// Returns the snapshot type resolution failure, if that was the cause.
    pub fn snapshot_type_source(&self) -> Option<&SnapshotTypeSource> {
        match self {
            Self::SnapshotType { source } => Some(source),
            Self::Payload { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests;
