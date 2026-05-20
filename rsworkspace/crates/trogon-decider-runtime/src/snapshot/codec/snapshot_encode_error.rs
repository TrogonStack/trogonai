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
