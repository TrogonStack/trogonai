use tokio_tungstenite::tungstenite;

use crate::Error;

/// A failure of [`crate::WebSocketTransport`]. `Display` is the text the TypeScript browser SDK's WebSocket
/// transport raises; the transport hands it to the SDK as [`Error::Transport`].
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum WebSocketError {
    /// `new URL(url)` threw in the TypeScript transport.
    #[error("Invalid URL")]
    InvalidUrl,
    #[error("WebSocket URL must use ws:// or wss:// protocol")]
    UnsupportedScheme,
    #[error("WebSocket connection timeout after 15000ms")]
    ConnectTimeout,
    /// The handshake or the connection failed.
    #[error("WebSocket connection error")]
    Connection,
    /// The peer closed with a code other than 1000 and 1001, or dropped the connection (code 1006, no reason).
    #[error("WebSocket closed abnormally with code {code}: {reason}")]
    ClosedAbnormally { code: u16, reason: String },
    #[error("WebSocket connection aborted by user")]
    Aborted,
    #[error("WebSocketTransport is not ready for writing")]
    NotReady,
    /// A handshake header that [`crate::HeaderDelivery::Handshake`] could not put on the upgrade request.
    #[error("invalid WebSocket handshake header: {name}")]
    InvalidHeader { name: String },
}

impl From<tungstenite::Error> for WebSocketError {
    fn from(_: tungstenite::Error) -> Self {
        Self::Connection
    }
}

impl From<WebSocketError> for Error {
    fn from(error: WebSocketError) -> Self {
        Self::Transport(Box::new(error))
    }
}
