#[derive(Debug)]
pub struct SnapshotEncodeError<Source> {
    source: Source,
}

impl<Source> SnapshotEncodeError<Source> {
    pub(super) fn new(source: Source) -> Self {
        Self { source }
    }

    pub fn source(&self) -> &Source {
        &self.source
    }
}

impl<Source> std::fmt::Display for SnapshotEncodeError<Source>
where
    Source: std::fmt::Display,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to encode snapshot payload: {}", self.source)
    }
}

impl<Source> std::error::Error for SnapshotEncodeError<Source>
where
    Source: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
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
