use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Duration;

use trogon_credential_domain::{CredentialKey, InjectionLocation};

use super::constants::RELEASE_CACHE_JITTER;
use super::{ReleaseDestination, ReleaseRequest, TenantAccount};

/// What makes two releases the same one: the tenant, the credential, the
/// exact destination, and the location in the outbound request.
///
/// The correlation id names one use, not the release, so it is not part of
/// the key.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ReleaseCacheKey {
    tenant: TenantAccount,
    credential: CredentialKey,
    destination: ReleaseDestination,
    location: InjectionLocation,
}

impl ReleaseCacheKey {
    pub(crate) fn of(release: &ReleaseRequest) -> Self {
        Self {
            tenant: release.tenant().clone(),
            credential: release.credential().clone(),
            destination: release.destination().clone(),
            location: release.location().clone(),
        }
    }

    pub(crate) fn tenant(&self) -> &TenantAccount {
        &self.tenant
    }

    pub(crate) fn credential(&self) -> &CredentialKey {
        &self.credential
    }

    /// How far ahead of a hold `ttl` away this key's copy expires: the same
    /// for the same key, and below both [`RELEASE_CACHE_JITTER`] and `ttl`,
    /// as the gateway spreads its own expiries.
    pub(crate) fn jitter_within(&self, ttl: Duration) -> Duration {
        let bound = RELEASE_CACHE_JITTER.min(ttl);
        if bound.is_zero() {
            return Duration::ZERO;
        }
        let mut hasher = DefaultHasher::new();
        self.tenant.hash(&mut hasher);
        self.credential.to_string().hash(&mut hasher);
        self.destination.hash(&mut hasher);
        self.location.to_string().hash(&mut hasher);
        let offset = u128::from(hasher.finish()) % bound.as_nanos();
        Duration::from_nanos(u64::try_from(offset).unwrap_or(u64::MAX))
    }
}

#[cfg(test)]
mod tests;
