#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! Credential value objects shared by every credential consumer.
//!
//! A credential is named by its [`CredentialIdentity`], recorded once on the
//! write request that creates it, and addressed afterwards by its
//! [`CredentialKey`]. A [`CredentialRef`] pins one version of its material, and
//! [`CredentialPath`] is where that material lives in the managed subtree.
//! [`PublicCredentialId`] is the identifier callers see, composed from validated
//! identifiers so no storage artifact reaches a wire contract.
//!
//! A strict release check pairs a [`ConfiguredReleasePolicy`] with a
//! [`CompleteReleaseRequest`], so neither the policy nor the request can leave a
//! dimension unstated and have it read as a wildcard.
//!
//! Every type validates on construction and carries no transport, serialization,
//! or storage concern, so both sides of the
//! [ADR#0074](https://github.com/TrogonStack/trogonai/blob/main/docs/adr/0074-credential-platform-extraction-boundary.md)
//! extraction boundary can share them.

mod allowed_host;
mod complete_release_request;
mod configured_release_policy;
mod constants;
mod credential_destroy_reason;
mod credential_failure_reason;
mod credential_fingerprint;
mod credential_id;
mod credential_identity;
mod credential_identity_version;
mod credential_key;
mod credential_kind;
mod credential_metadata;
mod credential_owner_id;
mod credential_path;
mod credential_ref;
mod credential_scope;
mod credential_status;
mod credential_version;
mod host_syntax;
mod injection_location;
mod public_credential_id;
mod release_host;
mod runtime_release_policy;
mod runtime_service_id;
mod source_integration_id;
mod source_kind;

pub use allowed_host::{AllowedHost, AllowedHostError, AllowedHosts};
pub use complete_release_request::CompleteReleaseRequest;
pub use configured_release_policy::{ConfiguredReleasePolicy, ConfiguredReleasePolicyError};
pub use credential_destroy_reason::{CredentialDestroyReason, CredentialDestroyReasonError};
pub use credential_failure_reason::{CredentialFailureReason, CredentialFailureReasonError};
pub use credential_fingerprint::{CredentialFingerprint, CredentialFingerprintError};
pub use credential_id::{CredentialId, CredentialIdError};
pub use credential_identity::CredentialIdentity;
pub use credential_identity_version::CredentialIdentityVersion;
pub use credential_key::CredentialKey;
pub use credential_kind::CredentialKind;
pub use credential_metadata::CredentialMetadata;
pub use credential_owner_id::{CredentialOwnerId, CredentialOwnerIdError};
pub use credential_path::CredentialPath;
pub use credential_ref::CredentialRef;
pub use credential_scope::CredentialScope;
pub use credential_status::CredentialStatus;
pub use credential_version::{CredentialVersion, CredentialVersionError};
pub use injection_location::{InjectionLocation, InjectionLocationError, InjectionLocations};
pub use public_credential_id::PublicCredentialId;
pub use release_host::{ReleaseHost, ReleaseHostError};
pub use runtime_release_policy::{
    RuntimeReleaseDeniedError, RuntimeReleasePolicy, RuntimeReleasePolicyError, RuntimeReleaseRequest,
};
pub use runtime_service_id::{AllowedRuntimeServices, RuntimeServiceId, RuntimeServiceIdError};
pub use source_integration_id::{SourceIntegrationId, SourceIntegrationIdError};
pub use source_kind::SourceKind;
