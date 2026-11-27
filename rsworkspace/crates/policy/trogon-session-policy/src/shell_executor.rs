use crate::ShellGrant;

/// Start only through an accepted VM worker. A native host shell is not an adapter.
/// Enforce the grant's resource, filesystem, network and placement limits before execution.
#[async_trait::async_trait]
pub trait ShellExecutor: Send {
    type Output;
    type Error: std::error::Error + Send + Sync + 'static;
    async fn shell(&mut self, grant: &ShellGrant, command: &str) -> Result<Self::Output, Self::Error>;
}
