/// Why the broker could not be asked.
#[derive(Debug, thiserror::Error)]
pub enum OracleError {
    /// Nothing subscribes the verb, so ownership could not be verified.
    #[error("no responders for the authority verb")]
    NoResponders,
    /// The broker did not answer within the request timeout.
    #[error("authority request timed out")]
    TimedOut,
    /// The request could not be encoded.
    #[error("authority request could not be encoded: {0}")]
    Encode(serde_json::Error),
    /// The answer could not be read.
    #[error("authority answer could not be read: {0}")]
    Decode(serde_json::Error),
    /// The request failed in transit.
    #[error("authority request failed: {0}")]
    Request(async_nats::RequestError),
    /// Another oracle failed.
    #[error("{0}")]
    Failed(Box<dyn std::error::Error + Send + Sync>),
}
