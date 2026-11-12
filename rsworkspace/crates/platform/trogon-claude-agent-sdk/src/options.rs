use std::{num::NonZeroUsize, path::PathBuf};

use crate::{AgentName, EffortLevel, Environment, ModelName, PermissionMode, SettingSource, ToolRule, UsdBudget};

#[derive(Clone, Debug, Default)]
pub struct ClaudeAgentOptions {
    pub cli_path: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub model: Option<ModelName>,
    pub permission_mode: Option<PermissionMode>,
    /// Filesystem settings to load: `None` inherits CLI defaults; an empty list disables them.
    pub setting_sources: Option<Vec<SettingSource>>,
    pub max_turns: Option<NonZeroUsize>,
    pub max_budget_usd: Option<UsdBudget>,
    pub effort: Option<EffortLevel>,
    pub env: Environment,
    pub additional_directories: Vec<PathBuf>,
    pub agent: Option<AgentName>,
    pub allowed_tools: Vec<ToolRule>,
}
