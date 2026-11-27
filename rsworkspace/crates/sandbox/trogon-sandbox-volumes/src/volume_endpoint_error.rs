/// A string is not an `http` or `https` object-store endpoint.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum VolumeEndpointError {
    #[error("endpoint is not a valid URL: {0}")]
    Malformed(#[from] url::ParseError),
    #[error("endpoint scheme '{0}' must be http or https")]
    UnsupportedScheme(String),
}
