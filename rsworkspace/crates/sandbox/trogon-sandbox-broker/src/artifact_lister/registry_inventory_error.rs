/// Why a registry inventory read failed.
#[derive(Debug, thiserror::Error)]
pub enum RegistryInventoryError {
    #[error("registry host is invalid: {0}")]
    Host(#[from] trogon_sandbox_oci::RegistryHostError),
    #[error("registry tag listing failed: {0}")]
    Registry(#[from] trogon_sandbox_oci::OciError),
}
