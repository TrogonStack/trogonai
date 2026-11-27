/// Reasons a WebSocket gateway URL is rejected.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WsGatewayUrlError {
    #[error("gateway url is not a valid url: {0}")]
    Malformed(#[from] url::ParseError),
    #[error("gateway url scheme must be ws or wss, got {0}")]
    UnsupportedScheme(String),
}
