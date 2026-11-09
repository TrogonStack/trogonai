use std::path::PathBuf;

use crate::{PermissionMode, SettingSource};

#[derive(Clone, Debug, Default)]
pub struct ClaudeAgentOptions {
    pub cli_path: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
    pub permission_mode: Option<PermissionMode>,
    /// Filesystem settings to load: `None` inherits CLI defaults; an empty list disables them.
    pub setting_sources: Option<Vec<SettingSource>>,
}
