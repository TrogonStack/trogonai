#[derive(Debug)]
pub struct SnapshotDecodeError<Source> {
    source: Source,
}

impl<Source> SnapshotDecodeError<Source> {
    pub(super) fn new(source: Source) -> Self {
        Self { source }
    }

    pub fn source(&self) -> &Source {
        &self.source
    }
}

impl<Source> std::fmt::Display for SnapshotDecodeError<Source>
where
    Source: std::fmt::Display,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to decode snapshot payload: {}", self.source)
    }
}

impl<Source> std::error::Error for SnapshotDecodeError<Source>
where
    Source: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}
