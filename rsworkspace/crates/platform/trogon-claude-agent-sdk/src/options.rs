use std::{num::NonZeroUsize, path::PathBuf};

use crate::{EffortLevel, PermissionMode, SettingSource, UsdBudget};

#[derive(Clone, Debug, Default)]
pub struct ClaudeAgentOptions {
    pub cli_path: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
    pub permission_mode: Option<PermissionMode>,
    /// Filesystem settings to load: `None` inherits CLI defaults; an empty list disables them.
    pub setting_sources: Option<Vec<SettingSource>>,
    pub max_turns: Option<NonZeroUsize>,
    pub max_budget_usd: Option<UsdBudget>,
    pub effort: Option<EffortLevel>,
}
