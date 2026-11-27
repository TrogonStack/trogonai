use async_trait::async_trait;

/// Checks that a freshly ready guest answers commands.
#[async_trait]
pub trait GuestProbe: Send + Sync {
    /// Succeeds when VM `vm_name` in `namespace` runs a command.
    async fn ready(&self, namespace: &str, vm_name: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}
