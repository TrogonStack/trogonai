use serde::Deserialize;

/// What a command run inside a sandbox printed and how it ended.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}
