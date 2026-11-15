#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! Secret store ports for credential material, and the OpenBao adapter that
//! implements them.
//!
//! Each operation is its own trait ([`SecretStorePut`], [`SecretStoreGet`],
//! [`SecretStoreRotate`], [`SecretStoreRevoke`], [`SecretStoreMetadata`], and
//! [`SecretStoreDestroy`]), so a caller depends only on the operations it uses.
//! [`OpenBaoSecretStore`] implements every port against an OpenBao KV v2 mount
//! with a token it is handed. [`OpenBaoReleaseReader`] implements
//! [`SecretStoreCorrelatedGet`] for the secrets service: it logs in with an
//! [`OpenBaoAppRole`], keeps that login alive, and reads each credential with a
//! child token minted for that read alone.
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

mod constants;
mod fingerprint;
mod generated_value_error;
#[cfg(any(test, feature = "test-support"))]
mod in_memory_secret_store;
#[cfg(any(test, feature = "test-support"))]
mod mock_openbao_secret_store;
mod openbao_app_role;
mod openbao_app_role_session;
mod openbao_auth_response;
mod openbao_call_error;
mod openbao_caller;
mod openbao_child_tokens;
#[cfg(test)]
mod openbao_harness;
mod openbao_http;
mod openbao_lease;
mod openbao_login_error;
mod openbao_name_error;
mod openbao_policy_name;
mod openbao_release_reader;
mod openbao_secret_store;
mod openbao_secret_store_config_error;
mod openbao_token_role;
mod openbao_token_source;
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
pub use openbao_app_role::{OpenBaoAppRole, OpenBaoAppRoleError};
pub use openbao_child_tokens::OpenBaoChildTokens;
pub use openbao_login_error::OpenBaoLoginError;
pub use openbao_name_error::OpenBaoNameError;
pub use openbao_policy_name::OpenBaoPolicyName;
pub use openbao_release_reader::OpenBaoReleaseReader;
pub use openbao_secret_store::{OpenBaoMount, OpenBaoMountError, OpenBaoSecretStore, OpenBaoTransport};
pub use openbao_secret_store_config_error::OpenBaoSecretStoreConfigError;
pub use openbao_token_role::OpenBaoTokenRole;
pub use secret_destroy_reason::{SecretDestroyReason, SecretDestroyReasonError};
pub use secret_material::SecretMaterial;
pub use secret_store_error::SecretStoreError;
pub use secret_verifier::{SecretVerifier, SecretVerifierError};
pub use traits::{
    SecretStoreCorrelatedGet, SecretStoreDestroy, SecretStoreGet, SecretStoreMetadata, SecretStorePut,
    SecretStoreRevoke, SecretStoreRotate,
};

use openbao_app_role_session::OpenBaoAppRoleSession;
use openbao_auth_response::OpenBaoAuthResponse;
use openbao_call_error::OpenBaoCallError;
use openbao_caller::OpenBaoCaller;
use openbao_lease::{OpenBaoLease, OpenBaoLeaseRenewal};
use openbao_token_source::OpenBaoTokenSource;
