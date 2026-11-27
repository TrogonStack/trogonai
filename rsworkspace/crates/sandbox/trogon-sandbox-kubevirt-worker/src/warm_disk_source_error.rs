use std::error::Error;

/// A [`crate::WarmDiskSource`] could not describe the warm VM's root disk.
#[derive(Debug, thiserror::Error)]
#[error("the warm disk source failed: {source}")]
pub struct WarmDiskSourceError {
    #[source]
    source: Box<dyn Error + Send + Sync>,
}

impl WarmDiskSourceError {
    pub fn new(source: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self { source: source.into() }
    }
}
