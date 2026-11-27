use trogon_sandbox_contract::FileOpErrorKind;
use trogon_sandbox_file_ops::FileOpError;

use super::constants::{EXIT_IS_DIRECTORY, STDERR_EXCERPT_CHARS};
use super::stderr_kind::stderr_kind;
use crate::envd::ProcessResult;

/// The first characters of the guest's stderr.
pub(super) fn stderr_excerpt(output: &ProcessResult) -> String {
    String::from_utf8_lossy(&output.stderr)
        .chars()
        .take(STDERR_EXCERPT_CHARS)
        .collect()
}

/// The failure a guest script that exited nonzero stands for, named `label` in the message.
pub(super) fn script_failure(label: &str, output: &ProcessResult) -> FileOpError {
    let message = stderr_excerpt(output);
    let kind = if output.exit_code == EXIT_IS_DIRECTORY {
        FileOpErrorKind::IsADirectory
    } else {
        stderr_kind(&message)
    };
    FileOpError::failed(kind, format!("{label} failed (exit {}): {message}", output.exit_code))
}
