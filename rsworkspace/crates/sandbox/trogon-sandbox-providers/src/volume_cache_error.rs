/// Why the clone cache could not be used.
#[derive(Debug, thiserror::Error)]
pub enum VolumeCacheError {
    #[error("volume-cache enabled but SANDBOX_VOLUME_CACHE_BUCKET is unset")]
    BucketUnset,
    #[error("invalid volume cache setting {variable}: {source}")]
    InvalidSetting {
        variable: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("volume cache client: {0}")]
    Client(#[source] Box<dyn std::error::Error + Send + Sync>),
}
