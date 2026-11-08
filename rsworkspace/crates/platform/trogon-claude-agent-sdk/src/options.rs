use std::path::PathBuf;

use crate::{CliExecutable, ModelName};

#[derive(Clone, Debug, Default)]
pub struct ClaudeAgentOptions {
    pub executable: CliExecutable,
    pub cwd: Option<PathBuf>,
    pub model: Option<ModelName>,
}
