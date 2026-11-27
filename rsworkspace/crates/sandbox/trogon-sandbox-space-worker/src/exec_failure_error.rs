/// Why a command did not produce a result.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum ExecFailureError {
    /// SPACE refused the command or was never reached, so nothing ran.
    #[error("SPACE did not run the command: {0}")]
    NotStarted(String),
    /// The command may have started. It must not be sent again.
    #[error("SPACE command outcome is unknown ({0}); it was not retried")]
    Unknown(String),
}
