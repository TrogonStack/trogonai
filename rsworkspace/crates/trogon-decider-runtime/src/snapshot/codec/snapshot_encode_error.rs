#[derive(Debug, thiserror::Error)]
pub enum SnapshotEncodeError<PayloadSource, SnapshotTypeSource = std::convert::Infallible> {
    #[error("failed to resolve snapshot type: {source}")]
    SnapshotType {
        #[source]
        source: SnapshotTypeSource,
    },
    #[error("failed to encode snapshot payload: {source}")]
    Payload {
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

    pub fn payload_source(&self) -> Option<&PayloadSource> {
        match self {
            Self::Payload { source } => Some(source),
            Self::SnapshotType { .. } => None,
        }
    }

    pub fn snapshot_type_source(&self) -> Option<&SnapshotTypeSource> {
        match self {
            Self::SnapshotType { source } => Some(source),
            Self::Payload { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests;
