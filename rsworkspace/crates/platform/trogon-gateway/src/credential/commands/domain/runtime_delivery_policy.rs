use std::time::Duration;

use super::{AllowedHosts, AllowedRuntimeServices, InjectionLocation, InjectionLocations, RuntimeServiceId};

/// The delivery policy the runtime projection carries per integration.
///
/// A `workspace_id` field is deliberately absent: ADR#0046 collapses
/// workspace-shaped fields into the project, and the projection's existing
/// `owner_id` already carries the project id.
///
/// The default is permissive on hosts and runtime services because nothing can
/// populate these yet: there is no management API to configure them, and the
/// credential event stream does not carry them. A restrictive default would
/// take every shipped source offline. Once a value *is* set, enforcement is
/// fail-closed, so the permissive default is confined to the unconfigured case.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeDeliveryPolicy {
    allowed_hosts: AllowedHosts,
    allowed_runtime_services: AllowedRuntimeServices,
    injection_locations: InjectionLocations,
    cache_ttl_override: Option<Duration>,
}

impl RuntimeDeliveryPolicy {
    pub fn with_allowed_hosts(mut self, allowed_hosts: AllowedHosts) -> Self {
        self.allowed_hosts = allowed_hosts;
        self
    }

    pub fn with_allowed_runtime_services(mut self, allowed_runtime_services: AllowedRuntimeServices) -> Self {
        self.allowed_runtime_services = allowed_runtime_services;
        self
    }

    pub fn with_injection_locations(mut self, injection_locations: InjectionLocations) -> Self {
        self.injection_locations = injection_locations;
        self
    }

    pub fn with_cache_ttl_override(mut self, ttl: Duration) -> Result<Self, RuntimeDeliveryPolicyError> {
        if ttl.is_zero() {
            return Err(RuntimeDeliveryPolicyError::ZeroCacheTtl);
        }
        self.cache_ttl_override = Some(ttl);
        Ok(self)
    }

    pub fn allowed_hosts(&self) -> &AllowedHosts {
        &self.allowed_hosts
    }

    pub fn allowed_runtime_services(&self) -> &AllowedRuntimeServices {
        &self.allowed_runtime_services
    }

    pub fn injection_locations(&self) -> &InjectionLocations {
        &self.injection_locations
    }

    pub fn cache_ttl_override(&self) -> Option<Duration> {
        self.cache_ttl_override
    }

    /// A policy that never narrows below the configured cache TTL cannot be
    /// used to hold a revoked credential past the ADR#0049 staleness bound, so
    /// an override is only honoured when it shortens the window.
    pub fn effective_cache_ttl(&self, default_ttl: Duration) -> Duration {
        match self.cache_ttl_override {
            Some(override_ttl) => override_ttl.min(default_ttl),
            None => default_ttl,
        }
    }

    pub fn permits(&self, request: &RuntimeDeliveryRequest<'_>) -> Result<(), RuntimeDeliveryDenied> {
        if !self.allowed_runtime_services.permits(request.runtime_service) {
            return Err(RuntimeDeliveryDenied::RuntimeService {
                runtime_service: request.runtime_service.cloned(),
            });
        }
        if !self.allowed_hosts.permits(request.host) {
            return Err(RuntimeDeliveryDenied::Host {
                host: request.host.map(str::to_string),
            });
        }
        if let Some(location) = request.injection_location
            && !self.injection_locations.permits(location)
        {
            return Err(RuntimeDeliveryDenied::InjectionLocation {
                location: location.clone(),
            });
        }
        Ok(())
    }
}

/// What a caller is asking to do with a resolved credential.
///
/// Every field is optional because the shipped webhook paths resolve without
/// an outbound target. An absent field is not a wildcard: a configured
/// restriction denies when the matching field is missing.
#[derive(Clone, Copy, Debug, Default)]
pub struct RuntimeDeliveryRequest<'a> {
    runtime_service: Option<&'a RuntimeServiceId>,
    host: Option<&'a str>,
    injection_location: Option<&'a InjectionLocation>,
}

impl<'a> RuntimeDeliveryRequest<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn by_runtime_service(mut self, runtime_service: &'a RuntimeServiceId) -> Self {
        self.runtime_service = Some(runtime_service);
        self
    }

    pub fn to_host(mut self, host: &'a str) -> Self {
        self.host = Some(host);
        self
    }

    pub fn at_injection_location(mut self, location: &'a InjectionLocation) -> Self {
        self.injection_location = Some(location);
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RuntimeDeliveryDenied {
    #[error("runtime service {} is not allowed to resolve this credential", describe_service(.runtime_service.as_ref()))]
    RuntimeService { runtime_service: Option<RuntimeServiceId> },
    #[error("host {} is not in the credential's allowed hosts", describe_host(.host.as_deref()))]
    Host { host: Option<String> },
    #[error("injection location {location} is not allowed for this credential")]
    InjectionLocation { location: InjectionLocation },
}

fn describe_service(runtime_service: Option<&RuntimeServiceId>) -> String {
    match runtime_service {
        Some(runtime_service) => format!("'{runtime_service}'"),
        None => "(unidentified)".to_string(),
    }
}

fn describe_host(host: Option<&str>) -> String {
    match host {
        Some(host) => format!("'{host}'"),
        None => "(unspecified)".to_string(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RuntimeDeliveryPolicyError {
    #[error("cache ttl override must be greater than zero")]
    ZeroCacheTtl,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(value: &str) -> RuntimeServiceId {
        RuntimeServiceId::new(value).unwrap()
    }

    #[test]
    fn default_policy_permits_an_unqualified_request() {
        let policy = RuntimeDeliveryPolicy::default();

        assert_eq!(policy.permits(&RuntimeDeliveryRequest::new()), Ok(()));
    }

    #[test]
    fn host_restriction_denies_an_unspecified_host() {
        let policy =
            RuntimeDeliveryPolicy::default().with_allowed_hosts(AllowedHosts::only(["api.example.com"]).unwrap());

        assert_eq!(
            policy.permits(&RuntimeDeliveryRequest::new()),
            Err(RuntimeDeliveryDenied::Host { host: None })
        );
        assert_eq!(
            policy.permits(&RuntimeDeliveryRequest::new().to_host("api.example.com")),
            Ok(())
        );
        assert_eq!(
            policy.permits(&RuntimeDeliveryRequest::new().to_host("evil.example.net")),
            Err(RuntimeDeliveryDenied::Host {
                host: Some("evil.example.net".to_string())
            })
        );
    }

    #[test]
    fn runtime_service_restriction_denies_an_unidentified_caller() {
        let policy = RuntimeDeliveryPolicy::default()
            .with_allowed_runtime_services(AllowedRuntimeServices::only(["trogon-gateway"]).unwrap());
        let allowed = service("trogon-gateway");
        let other = service("some-other-worker");

        assert_eq!(
            policy.permits(&RuntimeDeliveryRequest::new()),
            Err(RuntimeDeliveryDenied::RuntimeService { runtime_service: None })
        );
        assert_eq!(
            policy.permits(&RuntimeDeliveryRequest::new().by_runtime_service(&allowed)),
            Ok(())
        );
        assert_eq!(
            policy.permits(&RuntimeDeliveryRequest::new().by_runtime_service(&other)),
            Err(RuntimeDeliveryDenied::RuntimeService {
                runtime_service: Some(other)
            })
        );
    }

    #[test]
    fn service_identity_is_checked_before_the_host() {
        let policy = RuntimeDeliveryPolicy::default()
            .with_allowed_hosts(AllowedHosts::only(["api.example.com"]).unwrap())
            .with_allowed_runtime_services(AllowedRuntimeServices::only(["trogon-gateway"]).unwrap());

        assert_eq!(
            policy.permits(&RuntimeDeliveryRequest::new().to_host("api.example.com")),
            Err(RuntimeDeliveryDenied::RuntimeService { runtime_service: None })
        );
    }

    #[test]
    fn injection_location_is_only_checked_when_requested() {
        let header = InjectionLocation::header("authorization").unwrap();
        let query = InjectionLocation::query_parameter("token").unwrap();
        let policy =
            RuntimeDeliveryPolicy::default().with_injection_locations(InjectionLocations::new([header.clone()]));

        assert_eq!(policy.permits(&RuntimeDeliveryRequest::new()), Ok(()));
        assert_eq!(
            policy.permits(&RuntimeDeliveryRequest::new().at_injection_location(&header)),
            Ok(())
        );
        assert_eq!(
            policy.permits(&RuntimeDeliveryRequest::new().at_injection_location(&query)),
            Err(RuntimeDeliveryDenied::InjectionLocation { location: query })
        );
    }

    #[test]
    fn cache_ttl_override_may_only_shorten_the_window() {
        let default_ttl = Duration::from_secs(300);
        let shorter = RuntimeDeliveryPolicy::default()
            .with_cache_ttl_override(Duration::from_secs(60))
            .unwrap();
        let longer = RuntimeDeliveryPolicy::default()
            .with_cache_ttl_override(Duration::from_secs(3600))
            .unwrap();

        assert_eq!(
            RuntimeDeliveryPolicy::default().effective_cache_ttl(default_ttl),
            default_ttl
        );
        assert_eq!(shorter.effective_cache_ttl(default_ttl), Duration::from_secs(60));
        assert_eq!(longer.effective_cache_ttl(default_ttl), default_ttl);
    }

    #[test]
    fn rejects_a_zero_cache_ttl_override() {
        assert_eq!(
            RuntimeDeliveryPolicy::default().with_cache_ttl_override(Duration::ZERO),
            Err(RuntimeDeliveryPolicyError::ZeroCacheTtl)
        );
    }
}
