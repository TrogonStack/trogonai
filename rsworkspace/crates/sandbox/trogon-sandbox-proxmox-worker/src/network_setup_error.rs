use crate::ProxmoxApiError;

/// A guest's network could not be locked down to its proxy.
#[derive(Debug, thiserror::Error)]
pub enum NetworkSetupError {
    #[error(transparent)]
    Api(#[from] ProxmoxApiError),
    #[error("the guest address slot is missing")]
    MissingSlot,
    #[error("guest NIC anti-spoofing could not be verified: {0}")]
    AntiSpoof(String),
}
