use std::time::Duration;

use super::*;
use crate::{
    AllowedHosts, AllowedRuntimeServices, InjectionLocation, InjectionLocations, ReleaseHost, RuntimeServiceId,
};

fn authorization_header() -> InjectionLocation {
    InjectionLocation::header("authorization").unwrap()
}

fn full_policy() -> RuntimeReleasePolicy {
    RuntimeReleasePolicy::default()
        .with_allowed_runtime_services(AllowedRuntimeServices::only(["trogon-gateway"]).unwrap())
        .with_allowed_hosts(AllowedHosts::only(["api.example.com", "*.uploads.example.com"]).unwrap())
        .with_injection_locations(InjectionLocations::new([authorization_header()]))
}

fn configured() -> ConfiguredReleasePolicy {
    ConfiguredReleasePolicy::try_from(full_policy()).unwrap()
}

fn request(runtime_service: &str, host: &str, location: InjectionLocation) -> CompleteReleaseRequest {
    CompleteReleaseRequest::new(
        RuntimeServiceId::new(runtime_service).unwrap(),
        ReleaseHost::new(host).unwrap(),
        location,
    )
}

#[test]
fn refuses_the_default_policy() {
    assert_eq!(
        ConfiguredReleasePolicy::try_from(RuntimeReleasePolicy::default()),
        Err(ConfiguredReleasePolicyError::UnrestrictedRuntimeServices)
    );
}

#[test]
fn refuses_unrestricted_runtime_services() {
    let policy = full_policy().with_allowed_runtime_services(AllowedRuntimeServices::Unrestricted);

    assert_eq!(
        ConfiguredReleasePolicy::try_from(policy),
        Err(ConfiguredReleasePolicyError::UnrestrictedRuntimeServices)
    );
}

#[test]
fn refuses_an_empty_runtime_service_set() {
    let policy = full_policy().with_allowed_runtime_services(AllowedRuntimeServices::only(Vec::<&str>::new()).unwrap());

    assert_eq!(
        ConfiguredReleasePolicy::try_from(policy),
        Err(ConfiguredReleasePolicyError::NoRuntimeServices)
    );
}

#[test]
fn refuses_unrestricted_hosts() {
    let policy = full_policy().with_allowed_hosts(AllowedHosts::Unrestricted);

    assert_eq!(
        ConfiguredReleasePolicy::try_from(policy),
        Err(ConfiguredReleasePolicyError::UnrestrictedHosts)
    );
}

#[test]
fn refuses_an_empty_host_set() {
    let policy = full_policy().with_allowed_hosts(AllowedHosts::only(Vec::<&str>::new()).unwrap());

    assert_eq!(
        ConfiguredReleasePolicy::try_from(policy),
        Err(ConfiguredReleasePolicyError::NoHosts)
    );
}

#[test]
fn refuses_empty_injection_locations() {
    let policy = full_policy().with_injection_locations(InjectionLocations::default());

    assert_eq!(
        ConfiguredReleasePolicy::try_from(policy),
        Err(ConfiguredReleasePolicyError::NoInjectionLocations)
    );
}

#[test]
fn reports_the_first_failure_in_check_order() {
    let policy = RuntimeReleasePolicy::default()
        .with_allowed_runtime_services(AllowedRuntimeServices::only(["trogon-gateway"]).unwrap());

    assert_eq!(
        ConfiguredReleasePolicy::try_from(policy),
        Err(ConfiguredReleasePolicyError::UnrestrictedHosts)
    );
}

#[test]
fn accepts_a_fully_configured_policy() {
    assert!(ConfiguredReleasePolicy::try_from(full_policy()).is_ok());
}

#[test]
fn refusal_messages_name_the_missing_dimension() {
    assert_eq!(
        ConfiguredReleasePolicyError::NoInjectionLocations.to_string(),
        "release policy must name at least one injection location"
    );
}

#[test]
fn denies_a_mismatched_runtime_service() {
    let denied = request("some-other-worker", "api.example.com", authorization_header());

    assert_eq!(
        configured().permits(&denied),
        Err(RuntimeReleaseDeniedError::RuntimeService {
            runtime_service: Some(RuntimeServiceId::new("some-other-worker").unwrap())
        })
    );
}

#[test]
fn denies_a_mismatched_host() {
    let denied = request("trogon-gateway", "evil.example.net", authorization_header());

    assert_eq!(
        configured().permits(&denied),
        Err(RuntimeReleaseDeniedError::Host {
            host: Some("evil.example.net".to_string())
        })
    );
}

#[test]
fn denies_a_mismatched_injection_location() {
    let location = InjectionLocation::query_parameter("api_key").unwrap();
    let denied = request("trogon-gateway", "api.example.com", location.clone());

    assert_eq!(
        configured().permits(&denied),
        Err(RuntimeReleaseDeniedError::InjectionLocation { location })
    );
}

#[test]
fn permits_a_full_match() {
    let permitted = request("trogon-gateway", "api.example.com", authorization_header());

    assert_eq!(configured().permits(&permitted), Ok(()));
}

#[test]
fn a_subdomain_wildcard_matches_one_label_only() {
    let policy = configured();
    let check = |host: &str| policy.permits(&request("trogon-gateway", host, authorization_header()));

    assert_eq!(check("eu.uploads.example.com"), Ok(()));
    assert!(check("uploads.example.com").is_err());
    assert!(check("a.eu.uploads.example.com").is_err());
}

#[test]
fn effective_cache_ttl_honours_only_a_shortening_override() {
    let unset = configured();
    let overridden =
        ConfiguredReleasePolicy::try_from(full_policy().with_cache_ttl_override(Duration::from_secs(30)).unwrap())
            .unwrap();

    assert_eq!(
        unset.effective_cache_ttl(Duration::from_secs(60)),
        Duration::from_secs(60)
    );
    assert_eq!(
        overridden.effective_cache_ttl(Duration::from_secs(60)),
        Duration::from_secs(30)
    );
    assert_eq!(
        overridden.effective_cache_ttl(Duration::from_secs(10)),
        Duration::from_secs(10)
    );
}
