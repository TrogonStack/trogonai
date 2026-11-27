/// Why an event stream URL was refused.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum EventsUrlError {
    /// The text is not a URL.
    #[error("invalid events URL: {0}")]
    Invalid(#[from] url::ParseError),
    /// The URL points somewhere other than the data plane, so the attach token must
    /// not be sent there.
    #[error("events URL is not on the configured data-plane origin")]
    OffOrigin,
}
