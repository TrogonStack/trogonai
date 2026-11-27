/// Why a [`Hypervisor`](crate::Hypervisor) operation failed.
#[derive(Debug, thiserror::Error)]
pub enum HypervisorError {
    /// The host has no hypervisor backend.
    #[error("no hypervisor is available on this host")]
    Unavailable,
    /// The backend refused or failed the operation.
    #[error("{0}")]
    Failed(Box<dyn std::error::Error + Send + Sync>),
}

impl HypervisorError {
    /// Wraps a backend failure.
    pub fn failed(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self::Failed(error.into())
    }
}
