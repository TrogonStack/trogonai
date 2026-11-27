use crate::api::ApiError;

/// Why an envd terminal could not be served.
#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("the worker holds no E2B sandbox for this lease")]
    NotHeld,
    #[error("envd refused the terminal: {0}")]
    Envd(#[from] ApiError),
    #[error("the sandbox did not start the terminal process")]
    NotStarted,
    #[error("timed out waiting for the terminal process to start")]
    StartTimeout,
    #[error("the envd stream ended without an exit status")]
    StreamEnded,
    #[error("the terminal was closed")]
    Closed,
    #[error("the terminal output pipe failed: {0}")]
    Output(#[source] std::io::Error),
}
