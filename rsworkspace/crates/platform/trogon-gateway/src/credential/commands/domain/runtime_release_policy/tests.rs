use super::*;

fn service(value: &str) -> RuntimeServiceId {
    RuntimeServiceId::new(value).unwrap()
}

#[test]
fn default_policy_permits_an_unqualified_request() {
    let policy = RuntimeReleasePolicy::default();

    assert_eq!(policy.permits(&RuntimeReleaseRequest::new()), Ok(()));
}

#[test]
fn host_restriction_denies_an_unspecified_host() {
    let policy = RuntimeReleasePolicy::default().with_allowed_hosts(AllowedHosts::only(["api.example.com"]).unwrap());

    assert_eq!(
        policy.permits(&RuntimeReleaseRequest::new()),
        Err(RuntimeReleaseDeniedError::Host { host: None })
    );
    assert_eq!(
        policy.permits(&RuntimeReleaseRequest::new().to_host("api.example.com")),
        Ok(())
    );
    assert_eq!(
        policy.permits(&RuntimeReleaseRequest::new().to_host("evil.example.net")),
        Err(RuntimeReleaseDeniedError::Host {
            host: Some("evil.example.net".to_string())
        })
    );
}

#[test]
fn runtime_service_restriction_denies_an_unidentified_caller() {
    let policy = RuntimeReleasePolicy::default()
        .with_allowed_runtime_services(AllowedRuntimeServices::only(["trogon-gateway"]).unwrap());
    let allowed = service("trogon-gateway");
    let other = service("some-other-worker");

    assert_eq!(
        policy.permits(&RuntimeReleaseRequest::new()),
        Err(RuntimeReleaseDeniedError::RuntimeService { runtime_service: None })
    );
    assert_eq!(
        policy.permits(&RuntimeReleaseRequest::new().by_runtime_service(&allowed)),
        Ok(())
    );
    assert_eq!(
        policy.permits(&RuntimeReleaseRequest::new().by_runtime_service(&other)),
        Err(RuntimeReleaseDeniedError::RuntimeService {
            runtime_service: Some(other)
        })
    );
}

#[test]
fn service_identity_is_checked_before_the_host() {
    let policy = RuntimeReleasePolicy::default()
        .with_allowed_hosts(AllowedHosts::only(["api.example.com"]).unwrap())
        .with_allowed_runtime_services(AllowedRuntimeServices::only(["trogon-gateway"]).unwrap());

    assert_eq!(
        policy.permits(&RuntimeReleaseRequest::new().to_host("api.example.com")),
        Err(RuntimeReleaseDeniedError::RuntimeService { runtime_service: None })
    );
}

#[test]
fn injection_location_is_only_checked_when_requested() {
    let header = InjectionLocation::header("authorization").unwrap();
    let query = InjectionLocation::query_parameter("token").unwrap();
    let policy = RuntimeReleasePolicy::default().with_injection_locations(InjectionLocations::new([header.clone()]));

    assert_eq!(policy.permits(&RuntimeReleaseRequest::new()), Ok(()));
    assert_eq!(
        policy.permits(&RuntimeReleaseRequest::new().at_injection_location(&header)),
        Ok(())
    );
    assert_eq!(
        policy.permits(&RuntimeReleaseRequest::new().at_injection_location(&query)),
        Err(RuntimeReleaseDeniedError::InjectionLocation { location: query })
    );
}

#[test]
fn cache_ttl_override_may_only_shorten_the_window() {
    let default_ttl = Duration::from_secs(300);
    let shorter = RuntimeReleasePolicy::default()
        .with_cache_ttl_override(Duration::from_secs(60))
        .unwrap();
    let longer = RuntimeReleasePolicy::default()
        .with_cache_ttl_override(Duration::from_secs(3600))
        .unwrap();

    assert_eq!(
        RuntimeReleasePolicy::default().effective_cache_ttl(default_ttl),
        default_ttl
    );
    assert_eq!(shorter.effective_cache_ttl(default_ttl), Duration::from_secs(60));
    assert_eq!(longer.effective_cache_ttl(default_ttl), default_ttl);
}

#[test]
fn rejects_a_zero_cache_ttl_override() {
    assert_eq!(
        RuntimeReleasePolicy::default().with_cache_ttl_override(Duration::ZERO),
        Err(RuntimeReleasePolicyError::ZeroCacheTtl)
    );
}
