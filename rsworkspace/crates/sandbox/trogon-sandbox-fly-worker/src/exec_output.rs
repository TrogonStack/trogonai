use serde::Deserialize;

/// The result of a command run inside a Machine through the exec endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct ExecOutput {
    pub exit_code: i32,
    #[serde(default)]
    pub exit_signal: i32,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
}
