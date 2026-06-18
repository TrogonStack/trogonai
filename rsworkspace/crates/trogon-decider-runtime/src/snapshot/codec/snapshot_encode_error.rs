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
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("{0}")]
    struct TestSourceError(&'static str);

    #[test]
    fn display_and_source_preserve_payload_encode_error() {
        let error =
            SnapshotEncodeError::<TestSourceError, TestSourceError>::payload(TestSourceError("invalid payload"));

        assert_eq!(error.to_string(), "failed to encode snapshot payload: invalid payload");
        assert_eq!(
            std::error::Error::source(&error).map(ToString::to_string),
            Some("invalid payload".to_string())
        );
        assert!(error.payload_source().is_some());
        assert!(error.snapshot_type_source().is_none());
    }

    #[test]
    fn display_and_source_preserve_snapshot_type_error() {
        let error =
            SnapshotEncodeError::<TestSourceError, TestSourceError>::snapshot_type(TestSourceError("missing type"));

        assert_eq!(error.to_string(), "failed to resolve snapshot type: missing type");
        assert_eq!(
            std::error::Error::source(&error).map(ToString::to_string),
            Some("missing type".to_string())
        );
        assert!(error.payload_source().is_none());
        assert!(error.snapshot_type_source().is_some());
    }
}
