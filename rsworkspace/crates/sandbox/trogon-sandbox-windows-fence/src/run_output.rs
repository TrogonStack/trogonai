/// What one `fence-helper` run printed, trimmed. `status` is `None` when the
/// process ended without an exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}
