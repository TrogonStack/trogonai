#![allow(dead_code)]

mod credential_fingerprint;
mod credential_id;
mod credential_kind;
mod credential_metadata;
mod credential_owner_id;
mod credential_ref;
mod credential_scope;
mod credential_status;
mod credential_version;
mod in_memory_secret_store;
mod mock_openbao_secret_store;
pub(crate) mod openbao_secret_store;
mod secret_material;
mod secret_store_error;
mod secret_verifier;
mod source_kind;
mod static_config_secret_store;
mod storage_backend;
mod traits;

pub use credential_fingerprint::{CredentialFingerprint, CredentialFingerprintError};
pub use credential_id::{CredentialId, CredentialIdError};
pub use credential_kind::CredentialKind;
pub use credential_metadata::CredentialMetadata;
pub use credential_owner_id::CredentialOwnerId;
pub use credential_ref::CredentialRef;
pub use credential_scope::CredentialScope;
pub use credential_status::CredentialStatus;
pub use credential_version::CredentialVersion;
#[cfg(test)]
pub(crate) use mock_openbao_secret_store::MockOpenBaoSecretStore;
pub use secret_material::SecretMaterial;
pub use secret_store_error::SecretStoreError;
pub use secret_verifier::SecretVerifier;
pub use source_kind::SourceKind;
pub use static_config_secret_store::{StaticConfigSecretInput, StaticConfigSecretStore, StaticConfigSecretStoreError};
pub use storage_backend::StorageBackend;
pub use traits::{SecretStoreGet, SecretStoreMetadata, SecretStorePut, SecretStoreRevoke, SecretStoreRotate};
