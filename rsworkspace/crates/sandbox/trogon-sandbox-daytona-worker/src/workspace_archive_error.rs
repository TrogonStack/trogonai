use crate::DaytonaApiError;

/// Why archiving a directory inside a sandbox failed.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum WorkspaceArchiveError {
    #[error(transparent)]
    Api(#[from] DaytonaApiError),
    /// The in-sandbox `tar` command ended with a non-zero exit code.
    #[error("workspace tar build failed (exit {exit_code}): {output}")]
    TarFailed { exit_code: i32, output: String },
}
