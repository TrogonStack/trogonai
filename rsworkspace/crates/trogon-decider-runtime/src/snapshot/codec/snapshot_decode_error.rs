use crate::SnapshotTypeName;

#[derive(Debug, thiserror::Error)]
pub enum SnapshotDecodeError<PayloadSource, SnapshotTypeSource = std::convert::Infallible> {
    #[error("failed to resolve snapshot type: {source}")]
    SnapshotType {
        #[source]
        source: SnapshotTypeSource,
    },
    #[error("failed to decode snapshot payload: {source}")]
    Payload {
        #[source]
        source: PayloadSource,
    },
    #[error("unexpected snapshot type: expected {expected}, got {actual}")]
    UnexpectedType { expected: SnapshotTypeName, actual: String },
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

    pub fn payload_source(&self) -> Option<&PayloadSource> {
        match self {
            Self::Payload { source } => Some(source),
            Self::SnapshotType { .. } | Self::UnexpectedType { .. } => None,
        }
    }

    pub fn snapshot_type_source(&self) -> Option<&SnapshotTypeSource> {
        match self {
            Self::SnapshotType { source } => Some(source),
            Self::Payload { .. } | Self::UnexpectedType { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests;

