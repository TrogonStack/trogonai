use std::{collections::BTreeMap, num::NonZeroUsize, path::PathBuf};

use crate::{EffortLevel, PermissionMode, SettingSource, UsdBudget};

#[derive(Clone, Default)]
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
    pub env: BTreeMap<String, String>,
    pub additional_directories: Vec<PathBuf>,
}

impl std::fmt::Debug for ClaudeAgentOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClaudeAgentOptions")
            .field("cli_path", &self.cli_path)
            .field("cwd", &self.cwd)
            .field("permission_mode", &self.permission_mode)
            .field("setting_sources", &self.setting_sources)
            .field("max_turns", &self.max_turns)
            .field("max_budget_usd", &self.max_budget_usd)
            .field("effort", &self.effort)
            .finish_non_exhaustive()
    }
}
