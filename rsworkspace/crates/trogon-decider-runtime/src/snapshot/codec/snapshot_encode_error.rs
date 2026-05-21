type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug)]
pub struct SnapshotEncodeError {
    source: BoxError,
}

impl SnapshotEncodeError {
    pub(super) fn new<E>(source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self {
            source: Box::new(source),
        }
    }
}

impl std::fmt::Display for SnapshotEncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to encode snapshot payload: {}", self.source)
    }
}

impl std::error::Error for SnapshotEncodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct TestSourceError;

    impl std::fmt::Display for TestSourceError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("invalid payload")
        }
    }

    impl std::error::Error for TestSourceError {}

    #[test]
    fn display_and_source_preserve_payload_encode_error() {
        let error = SnapshotEncodeError::new(TestSourceError);

        assert_eq!(error.to_string(), "failed to encode snapshot payload: invalid payload");
        assert_eq!(
            std::error::Error::source(&error).map(ToString::to_string),
            Some("invalid payload".to_string())
        );
    }
}
