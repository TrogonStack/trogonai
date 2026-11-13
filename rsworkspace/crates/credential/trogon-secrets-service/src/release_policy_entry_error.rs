use std::time::Duration;

use trogon_credential_domain::{
    AllowedHostError, ConfiguredReleasePolicyError, CredentialIdError, CredentialOwnerIdError, InjectionLocationError,
    RuntimeReleasePolicyError, RuntimeServiceIdError,
};

use super::constants::RELEASE_TTL_BOUND;

/// Why one entry of the release policies file was refused.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ReleasePolicyEntryError {
    #[error("owner id is invalid")]
    InvalidOwner(#[source] CredentialOwnerIdError),
    #[error("credential id is invalid")]
    InvalidCredentialId(#[source] CredentialIdError),
    #[error("a runtime service is invalid")]
    InvalidRuntimeService(#[source] RuntimeServiceIdError),
    #[error("a host is invalid")]
    InvalidHost(#[source] AllowedHostError),
    #[error("an injection location is invalid")]
    InvalidLocation(#[source] InjectionLocationError),
    #[error("cache ttl is invalid")]
    InvalidCacheTtl(#[source] RuntimeReleasePolicyError),
    /// The release core never holds a release past [`RELEASE_TTL_BOUND`], so
    /// a longer cache ttl would be cut short without the operator knowing.
    #[error(
        "cache ttl of {}s is longer than the {}s a release may be held",
        ttl.as_secs(),
        RELEASE_TTL_BOUND.as_secs()
    )]
    CacheTtlAboveBound { ttl: Duration },
    #[error("policy does not restrict every dimension of a release")]
    NotConfigured(#[source] ConfiguredReleasePolicyError),
}
