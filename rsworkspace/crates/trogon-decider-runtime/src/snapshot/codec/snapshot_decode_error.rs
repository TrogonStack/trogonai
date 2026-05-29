use crate::SnapshotTypeName;

#[derive(Debug)]
pub enum SnapshotDecodeError<PayloadSource, SnapshotTypeSource = std::convert::Infallible> {
    SnapshotType { source: SnapshotTypeSource },
    Payload { source: PayloadSource },
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

impl<PayloadSource, SnapshotTypeSource> std::fmt::Display for SnapshotDecodeError<PayloadSource, SnapshotTypeSource>
where
    PayloadSource: std::fmt::Display,
    SnapshotTypeSource: std::fmt::Display,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SnapshotType { source } => write!(f, "failed to resolve snapshot type: {source}"),
            Self::Payload { source } => write!(f, "failed to decode snapshot payload: {source}"),
            Self::UnexpectedType { expected, actual } => {
                write!(f, "unexpected snapshot type: expected {expected}, got {actual}")
            }
        }
    }
}

impl<PayloadSource, SnapshotTypeSource> std::error::Error for SnapshotDecodeError<PayloadSource, SnapshotTypeSource>
where
    PayloadSource: std::error::Error + 'static,
    SnapshotTypeSource: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::SnapshotType { source } => Some(source),
            Self::Payload { source } => Some(source),
            Self::UnexpectedType { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct TestSourceError(&'static str);

    impl std::fmt::Display for TestSourceError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str(self.0)
        }
    }

    impl std::error::Error for TestSourceError {}

    #[test]
    fn display_and_source_preserve_payload_decode_error() {
        let error =
            SnapshotDecodeError::<TestSourceError, TestSourceError>::payload(TestSourceError("invalid payload"));

        assert_eq!(error.to_string(), "failed to decode snapshot payload: invalid payload");
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
            SnapshotDecodeError::<TestSourceError, TestSourceError>::snapshot_type(TestSourceError("missing type"));

        assert_eq!(error.to_string(), "failed to resolve snapshot type: missing type");
        assert_eq!(
            std::error::Error::source(&error).map(ToString::to_string),
            Some("missing type".to_string())
        );
        assert!(error.payload_source().is_none());
        assert!(error.snapshot_type_source().is_some());
    }

    #[test]
    fn display_reports_unexpected_snapshot_type() {
        let error = SnapshotDecodeError::<TestSourceError, TestSourceError>::unexpected_type(
            SnapshotTypeName::new("expected.snapshot").unwrap(),
            "actual.snapshot".to_string(),
        );

        assert_eq!(
            error.to_string(),
            "unexpected snapshot type: expected expected.snapshot, got actual.snapshot"
        );
        assert!(std::error::Error::source(&error).is_none());
        assert!(error.payload_source().is_none());
        assert!(error.snapshot_type_source().is_none());
    }
}
