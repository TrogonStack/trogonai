#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("unsupported protocol version")]
    UnsupportedVersion,
    #[error("invalid command arguments")]
    InvalidArguments,
    #[error("invalid child environment")]
    InvalidEnvironment,
    #[error("invalid working directory")]
    InvalidDirectory,
    #[error("invalid command deadline")]
    InvalidDeadline,
    #[error("invalid signal")]
    InvalidSignal,
    #[error("protocol frame exceeds its bound")]
    OversizedFrame,
    #[error("invalid protocol frame")]
    InvalidJson(#[source] serde_json::Error),
    #[error("protocol I/O failed")]
    Io(#[source] std::io::Error),
    #[error("protocol frame is unterminated")]
    UnterminatedFrame,
}
