use trogon_sandbox_contract::CommandEnv;

use crate::project_dir::ProjectDir;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LaunchCommand {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: ProjectDir,
    pub env: CommandEnv,
}
