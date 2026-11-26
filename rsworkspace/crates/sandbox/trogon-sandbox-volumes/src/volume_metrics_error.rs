/// Metric collectors could not be created, registered, or encoded.
#[derive(Debug, thiserror::Error)]
pub enum VolumeMetricsError {
    #[error("metrics registry: {0}")]
    Prometheus(#[from] prometheus::Error),
    #[error("encoded metrics are not valid UTF-8: {0}")]
    Encoding(#[from] std::string::FromUtf8Error),
}
