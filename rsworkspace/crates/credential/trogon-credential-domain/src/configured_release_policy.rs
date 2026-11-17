use std::time::Duration;

use super::{
    AllowedHosts, AllowedRuntimeServices, CompleteReleaseRequest, RuntimeReleaseDeniedError, RuntimeReleasePolicy,
    RuntimeReleaseRequest,
};

/// A release policy that restricts every dimension of a release.
///
/// It exists because absence is never a wildcard: an unrestricted or empty
/// dimension is refused when the policy is loaded, so a strict release check
/// can only ever run against a policy that names who may receive a credential,
/// where it may be sent, and where it may be placed. An empty dimension is a
/// misconfiguration rather than a deny-all, because a missing policy entry
/// already denies.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfiguredReleasePolicy(RuntimeReleasePolicy);

impl TryFrom<RuntimeReleasePolicy> for ConfiguredReleasePolicy {
    type Error = ConfiguredReleasePolicyError;

    fn try_from(policy: RuntimeReleasePolicy) -> Result<Self, Self::Error> {
        match policy.allowed_runtime_services() {
            AllowedRuntimeServices::Unrestricted => {
                return Err(ConfiguredReleasePolicyError::UnrestrictedRuntimeServices);
            }
            AllowedRuntimeServices::Only(services) if services.is_empty() => {
                return Err(ConfiguredReleasePolicyError::NoRuntimeServices);
            }
            AllowedRuntimeServices::Only(_) => {}
        }
        match policy.allowed_hosts() {
            AllowedHosts::Unrestricted => return Err(ConfiguredReleasePolicyError::UnrestrictedHosts),
            AllowedHosts::Only(hosts) if hosts.is_empty() => return Err(ConfiguredReleasePolicyError::NoHosts),
            AllowedHosts::Only(_) => {}
        }
        if policy.injection_locations().is_empty() {
            return Err(ConfiguredReleasePolicyError::NoInjectionLocations);
        }
        Ok(Self(policy))
    }
}

impl ConfiguredReleasePolicy {
    pub fn permits(&self, request: &CompleteReleaseRequest) -> Result<(), RuntimeReleaseDeniedError> {
        let request = RuntimeReleaseRequest::new()
            .by_runtime_service(request.runtime_service())
            .to_host(request.host().as_str())
            .at_injection_location(request.location());
        self.0.permits(&request)
    }

    pub fn effective_cache_ttl(&self, default_ttl: Duration) -> Duration {
        self.0.effective_cache_ttl(default_ttl)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConfiguredReleasePolicyError {
    #[error("release policy must restrict runtime services instead of leaving them unrestricted")]
    UnrestrictedRuntimeServices,
    #[error("release policy must name at least one runtime service")]
    NoRuntimeServices,
    #[error("release policy must restrict hosts instead of leaving them unrestricted")]
    UnrestrictedHosts,
    #[error("release policy must name at least one host")]
    NoHosts,
    #[error("release policy must name at least one injection location")]
    NoInjectionLocations,
}

#[cfg(test)]
mod tests;
