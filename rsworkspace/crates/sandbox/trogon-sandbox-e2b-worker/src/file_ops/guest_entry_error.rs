use trogon_sandbox_contract::FileOpErrorKind;
use trogon_sandbox_file_ops::FileOpError;

/// Why envd metadata could not be read as a file stat.
#[derive(Debug, thiserror::Error)]
pub(super) enum GuestEntryError {
    #[error("{0}")]
    Shape(#[from] serde_json::Error),
    #[error("invalid file size")]
    Size,
    #[error("invalid modification timestamp")]
    Timestamp,
}

impl From<GuestEntryError> for FileOpError {
    fn from(error: GuestEntryError) -> Self {
        Self::failed(FileOpErrorKind::Io, format!("invalid envd metadata: {error}"))
    }
}
