use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct ClaudeAgentOptions {
    pub cli_path: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
}
