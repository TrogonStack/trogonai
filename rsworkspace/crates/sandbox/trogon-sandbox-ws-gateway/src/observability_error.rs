use std::error::Error;

/// Reading the broker's KV state failed.
#[derive(Debug, thiserror::Error)]
pub enum ObservabilityError {
    #[error("opening KV bucket {bucket} failed")]
    Bucket {
        bucket: String,
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    #[error("reading KV entries failed")]
    Read(#[source] Box<dyn Error + Send + Sync>),
    #[error("a KV record did not decode")]
    Decode(#[from] serde_json::Error),
}

impl ObservabilityError {
    pub(crate) fn read(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Read(Box::new(source))
    }
}

#[cfg(test)]
mod tests;
