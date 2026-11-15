#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! Secret store ports for credential material, and the OpenBao adapter that
//! implements them.
//!
//! Each operation is its own trait ([`SecretStorePut`], [`SecretStoreGet`],
//! [`SecretStoreRotate`], [`SecretStoreRevoke`], [`SecretStoreMetadata`], and
//! [`SecretStoreDestroy`]), so a caller depends only on the operations it uses.
//! [`OpenBaoSecretStore`] implements every port against an OpenBao KV v2 mount.
//!
//! The traits are the
//! [ADR#0074](https://github.com/TrogonStack/trogonai/blob/main/docs/adr/0074-credential-platform-extraction-boundary.md)
//! extraction boundary: the OpenBao side moves into the secrets service, and
//! the gateway keeps the ports as a client of that service.
//!
//! # Features
//!
//! - `test-support`: adds `InMemorySecretStore` and `MockOpenBaoSecretStore`,
//!   in-process stores for tests that exercise the ports without a running
//!   OpenBao. Off by default; enable it only from `[dev-dependencies]`.

mod fingerprint;
mod generated_value_error;
#[cfg(any(test, feature = "test-support"))]
mod in_memory_secret_store;
#[cfg(any(test, feature = "test-support"))]
mod mock_openbao_secret_store;
mod openbao_secret_store;
mod secret_destroy_reason;
mod secret_material;
mod secret_store_error;
mod secret_verifier;
mod traits;

pub use generated_value_error::GeneratedValueError;
#[cfg(any(test, feature = "test-support"))]
pub use in_memory_secret_store::InMemorySecretStore;
#[cfg(any(test, feature = "test-support"))]
pub use mock_openbao_secret_store::MockOpenBaoSecretStore;
pub use openbao_secret_store::{
    OpenBaoMount, OpenBaoMountError, OpenBaoSecretStore, OpenBaoSecretStoreConfigError, OpenBaoTransport,
};
pub use secret_destroy_reason::{SecretDestroyReason, SecretDestroyReasonError};
pub use secret_material::SecretMaterial;
pub use secret_store_error::SecretStoreError;
pub use secret_verifier::{SecretVerifier, SecretVerifierError};
pub use traits::{
    SecretStoreDestroy, SecretStoreGet, SecretStoreMetadata, SecretStorePut, SecretStoreRevoke, SecretStoreRotate,
};
