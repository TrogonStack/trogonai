use std::path::PathBuf;

use crate::BusError;

/// Why a call failed without a typed answer from the broker.
///
/// Every variant leaves the outcome of the request unknown: the broker may have applied it before the failure.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("bus: {0}")]
    Bus(#[from] BusError),
    #[error("encoding the request failed: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("decoding the reply failed: {0}")]
    Decode(#[source] serde_json::Error),
    #[error("unexpected reply to {operation}: {reply}")]
    UnexpectedReply { operation: &'static str, reply: String },
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("{} failed: {source}", path.display())]
    Local {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} changed size while it was being transferred", path.display())]
    SizeChanged { path: PathBuf },
    #[error("the broker returned no bytes for {} before the end of the file", path.display())]
    EmptyChunk { path: PathBuf },
    #[error("{} ended before its expected size", path.display())]
    ShorterThanExpected { path: PathBuf },
    #[error("{} failed its integrity check", path.display())]
    IntegrityCheckFailed { path: PathBuf },
    #[error("{} escapes the transfer root", path.display())]
    PathEscapes { path: PathBuf },
    #[error("{} is not a regular file", path.display())]
    NotAFile { path: PathBuf },
    #[error("{} is not a directory", path.display())]
    NotADirectory { path: PathBuf },
    #[error("the sandbox needs `{tool}` for this transfer and it is not installed")]
    MissingDependency { tool: String },
}
