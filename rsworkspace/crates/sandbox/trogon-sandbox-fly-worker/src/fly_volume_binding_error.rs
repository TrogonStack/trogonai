use trogon_sandbox_contract::ProviderVolumeIdError;

/// A Fly volume could not be offered as a provider volume.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("the Fly volume id is not a valid provider volume id: {0}")]
pub struct FlyVolumeBindingError(#[from] pub ProviderVolumeIdError);
