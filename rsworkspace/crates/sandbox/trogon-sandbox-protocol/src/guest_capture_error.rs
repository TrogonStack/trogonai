/// Why the output of a guest capture script could not be read.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum GuestCaptureError {
    #[error("missing exit code")]
    MissingExitCode,
    #[error("invalid exit code")]
    InvalidExitCode,
    #[error("missing stream size")]
    MissingStreamSize,
    #[error("invalid stream size")]
    InvalidStreamSize,
    #[error("a captured stream is not valid base64: {0}")]
    Encoding(base64::DecodeError),
    #[error("guest returned more than the stream held")]
    MoreThanHeld,
}
