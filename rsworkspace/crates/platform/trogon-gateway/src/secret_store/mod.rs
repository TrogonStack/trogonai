#![allow(dead_code)]

mod in_memory_secret_store;
mod mock_openbao_secret_store;
pub(crate) mod openbao_secret_store;
mod secret_material;
mod secret_store_error;
mod secret_verifier;
mod static_config_secret_store;
mod traits;

#[cfg(test)]
pub(crate) use mock_openbao_secret_store::MockOpenBaoSecretStore;
pub use secret_material::SecretMaterial;
pub use secret_store_error::SecretStoreError;
pub use secret_verifier::SecretVerifier;
pub use static_config_secret_store::{StaticConfigSecretInput, StaticConfigSecretStore, StaticConfigSecretStoreError};
pub use traits::{SecretStoreGet, SecretStoreMetadata, SecretStorePut, SecretStoreRevoke, SecretStoreRotate};
