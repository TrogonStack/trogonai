use crate::policy::RootPolicyError;

/// Why the server stopped or could not start.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("invalid --deny-command or --allow-command pattern: {0}")]
    Commands(#[source] regex::Error),

    #[error("invalid --root: {0}")]
    Roots(#[source] RootPolicyError),

    #[error("the MCP handshake failed: {0}")]
    Serve(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("the MCP service ended abnormally: {0}")]
    Join(#[source] tokio::task::JoinError),
}
