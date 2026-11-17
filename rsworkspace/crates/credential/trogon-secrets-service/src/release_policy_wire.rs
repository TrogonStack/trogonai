use std::time::Duration;

use serde::Deserialize;
use trogon_credential_domain::{
    AllowedHosts, AllowedRuntimeServices, ConfiguredReleasePolicy, CredentialId, CredentialKey, CredentialOwnerId,
    InjectionLocation, InjectionLocations, RuntimeReleasePolicy,
};

use super::ReleasePolicyEntryError;
use super::constants::RELEASE_TTL_BOUND;

/// One release policy as written.
///
/// Runtime services and hosts have no default, so a policy cannot leave one
/// out and mean any. The injection locations are listed by kind, and a policy
/// must name at least one of them.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleasePolicyWire {
    owner_id: String,
    credential_id: String,
    runtime_services: Vec<String>,
    hosts: Vec<String>,
    #[serde(default)]
    headers: Vec<String>,
    #[serde(default)]
    query_parameters: Vec<String>,
    #[serde(default)]
    body_fields: Vec<String>,
    cache_ttl_secs: Option<u64>,
}

impl ReleasePolicyWire {
    /// The credential this entry is for, with its policy.
    pub(crate) fn into_policy(self) -> Result<(CredentialKey, ConfiguredReleasePolicy), ReleasePolicyEntryError> {
        let owner = CredentialOwnerId::new(&self.owner_id).map_err(ReleasePolicyEntryError::InvalidOwner)?;
        let credential_id =
            CredentialId::new(&self.credential_id).map_err(ReleasePolicyEntryError::InvalidCredentialId)?;
        let runtime_services = AllowedRuntimeServices::only(&self.runtime_services)
            .map_err(ReleasePolicyEntryError::InvalidRuntimeService)?;
        let hosts = AllowedHosts::only(&self.hosts).map_err(ReleasePolicyEntryError::InvalidHost)?;
        let locations = self
            .headers
            .iter()
            .map(InjectionLocation::header)
            .chain(self.query_parameters.iter().map(InjectionLocation::query_parameter))
            .chain(self.body_fields.iter().map(InjectionLocation::body_field))
            .collect::<Result<Vec<_>, _>>()
            .map_err(ReleasePolicyEntryError::InvalidLocation)?;
        let mut policy = RuntimeReleasePolicy::default()
            .with_allowed_runtime_services(runtime_services)
            .with_allowed_hosts(hosts)
            .with_injection_locations(InjectionLocations::new(locations));
        if let Some(secs) = self.cache_ttl_secs {
            let ttl = Duration::from_secs(secs);
            if ttl > RELEASE_TTL_BOUND {
                return Err(ReleasePolicyEntryError::CacheTtlAboveBound { ttl });
            }
            policy = policy
                .with_cache_ttl_override(ttl)
                .map_err(ReleasePolicyEntryError::InvalidCacheTtl)?;
        }
        let policy = ConfiguredReleasePolicy::try_from(policy).map_err(ReleasePolicyEntryError::NotConfigured)?;
        Ok((CredentialKey::new(owner, credential_id), policy))
    }
}
