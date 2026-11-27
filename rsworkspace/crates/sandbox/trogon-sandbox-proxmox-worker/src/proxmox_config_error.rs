use crate::ProxyNetworkError;

/// The worker settings are inconsistent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProxmoxConfigError {
    #[error("max leases must be between 1 and 128")]
    MaxLeases,
    #[error("the boot timeout must be positive")]
    BootTimeout,
    #[error(transparent)]
    Network(#[from] ProxyNetworkError),
}
