use crate::{Error, SseHttpError};

/// A failure of [`crate::SseTransport`]. `Display` is the text the TypeScript browser SDK's SSE transport raises;
/// the transport hands it to the SDK as [`Error::Transport`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SseError {
    #[error("SSE connection aborted by user")]
    Aborted,
    #[error("SSE connect timeout")]
    ConnectTimeout,
    #[error("SSE liveness timeout")]
    LivenessTimeout,
    #[error("SSE reconnect budget exhausted (5 attempts)")]
    BudgetExhausted,
    /// The server refused the stream or a write with a status code.
    #[error(transparent)]
    Http(#[from] SseHttpError),
    /// The request or the body stream failed; the text is what Node's `fetch` reports for the same failure.
    #[error("{0}")]
    Fetch(String),
    #[error("SSETransport is closed")]
    Closed,
    #[error("SSE transport closed before connect")]
    ClosedBeforeConnect,
    /// A written line was not JSON.
    #[error("{0}")]
    InvalidJson(String),
}

impl From<SseError> for Error {
    fn from(error: SseError) -> Self {
        Self::Transport(Box::new(error))
    }
}
