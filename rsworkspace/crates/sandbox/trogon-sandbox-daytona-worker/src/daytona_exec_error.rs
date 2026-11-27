use trogon_sandbox_protocol::GuestCaptureError;

use crate::DaytonaApiError;

/// Why running a command, or reading what it left behind, failed.
#[derive(Debug, thiserror::Error)]
pub enum DaytonaExecError {
    #[error(transparent)]
    Api(#[from] DaytonaApiError),
    #[error("the sandbox shell exited with status {0} instead of reporting a result")]
    ShellFailed(i32),
    #[error("the command's result could not be read: {0}")]
    Frame(#[from] GuestCaptureError),
    #[error("no process of the command was left to signal")]
    NoProcess,
}
