use trogon_sandbox_contract::CommandEnvError;

use crate::agent::AgentError;

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("{0}")]
    Unsupported(String),
    #[error("workspace preparation failed: {0}")]
    Failed(String),
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error("clone environment is not valid: {0}")]
    Env(#[from] CommandEnvError),
}
