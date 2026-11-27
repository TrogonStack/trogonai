use reqwest::StatusCode;
use trogon_sandbox_protocol::FileNotFoundError;

/// A call to the Fly Machines API failed.
#[derive(Debug, thiserror::Error)]
pub enum FlyApiError {
    #[error("the HTTP client could not be built: {0}")]
    Client(reqwest::Error),
    #[error("the Fly API request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("the Fly API endpoint cannot address {0}")]
    Url(#[from] url::ParseError),
    #[error("Fly API returned HTTP {0}")]
    Status(StatusCode),
    #[error("the Fly API response was not the expected JSON: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("{0} was not found")]
    NotFound(&'static str),
    #[error("snapshot create returned no id")]
    SnapshotWithoutId,
    #[error("a guest file read needs a non-empty path without NUL at a chunk boundary")]
    InvalidReadPosition,
    #[error("the guest file read failed with exit code {0}")]
    GuestRead(i32),
    #[error("the guest returned malformed base64: {0}")]
    Encoding(#[from] base64::DecodeError),
    #[error(transparent)]
    FileNotFound(FileNotFoundError),
}
