use std::path::PathBuf;

use crate::PermissionMode;

#[derive(Clone, Debug, Default)]
pub struct ClaudeAgentOptions {
    pub cli_path: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
    pub permission_mode: Option<PermissionMode>,
}
