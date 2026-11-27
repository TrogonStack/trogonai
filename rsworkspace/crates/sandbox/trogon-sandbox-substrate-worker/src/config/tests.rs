use std::time::Duration;

use trogon_sandbox_contract::{IsolationTier, SubstrateSizeClasses};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DIGEST_IMAGE: &str =
    "registry.example/actor-agent@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn config() -> Result<SubstrateConfig, Box<dyn std::error::Error>> {
    Ok(SubstrateConfig {
        ate_api_endpoint: "http://ate-api-server:8080".to_string(),
        router_endpoint: DEFAULT_ROUTER_ENDPOINT.parse()?,
        agent_image: DIGEST_IMAGE.to_string(),
        agent_port: 8110,
        sandbox_class: SandboxClass::Gvisor,
        snapshots_location: "gs://bucket/snapshots".to_string(),
        size_classes: "small=2048,medium=8192,large=32768".parse::<SubstrateSizeClasses>()?,
        template_build_timeout: Duration::from_secs(600),
        resume_timeout: Duration::from_secs(60),
        max_concurrent_leases: 64,
        router_tls: None,
        actor_cpu_millicores: 1000,
    })
}

#[test]
fn a_size_selector_picks_the_smallest_class_that_fits() -> TestResult {
    let selector = config()?.size_selector(4096).ok_or("a class fits")?;
    assert_eq!(
        selector.match_labels.get(SIZE_CLASS_LABEL).map(String::as_str),
        Some("medium")
    );
    Ok(())
}

#[test]
fn a_request_above_every_class_gets_no_selector_rather_than_an_empty_one() -> TestResult {
    assert!(config()?.size_selector(u32::MAX).is_none());
    Ok(())
}

#[test]
fn the_endpoint_is_the_shared_router_address() -> TestResult {
    assert_eq!(config()?.actor_endpoint("sbx-01j", "tenant-a"), DEFAULT_ROUTER_ENDPOINT);
    Ok(())
}

#[test]
fn resources_follow_the_selected_class() -> TestResult {
    let resources = config()?.resources_for(4096)?;
    assert_eq!(resources.memory_mib(), 8192);
    assert_eq!(resources.cpu_millicores(), 1000);
    Ok(())
}

#[test]
fn resources_for_a_request_above_every_class_are_refused() -> TestResult {
    assert_eq!(
        config()?.resources_for(u32::MAX).err(),
        Some(SubstrateConfigError::MemoryExceedsSizeClasses)
    );
    Ok(())
}

#[test]
fn resources_with_an_unusable_cpu_are_refused() -> TestResult {
    let mut config = config()?;
    config.actor_cpu_millicores = 0;
    assert!(matches!(
        config.resources_for(1024),
        Err(SubstrateConfigError::Resources(_))
    ));
    Ok(())
}

#[test]
fn a_well_formed_configuration_validates() -> TestResult {
    config()?.validate()?;
    Ok(())
}

#[test]
fn an_unpinned_agent_image_is_refused() -> TestResult {
    let mut config = config()?;
    config.agent_image = "registry.example/actor-agent:latest".to_string();
    assert!(matches!(config.validate(), Err(SubstrateConfigError::Image(_))));
    Ok(())
}

#[test]
fn a_zero_agent_port_is_refused() -> TestResult {
    let mut config = config()?;
    config.agent_port = 0;
    assert_eq!(config.validate(), Err(SubstrateConfigError::AgentPortZero));
    Ok(())
}

#[test]
fn a_cpu_request_that_no_class_can_carry_is_refused() -> TestResult {
    let mut config = config()?;
    config.actor_cpu_millicores = 0;
    assert!(matches!(config.validate(), Err(SubstrateConfigError::Resources(_))));
    Ok(())
}

#[test]
fn a_router_endpoint_without_a_port_is_rejected() {
    assert!(matches!(
        "atenet-router.ate-system.svc".parse::<RouterEndpoint>(),
        Err(RouterEndpointError::MissingPort { .. })
    ));
}

#[test]
fn a_router_endpoint_with_an_empty_host_is_rejected() {
    assert!(matches!(
        ":8444".parse::<RouterEndpoint>(),
        Err(RouterEndpointError::EmptyHost { .. })
    ));
}

#[test]
fn a_router_endpoint_with_a_non_numeric_port_is_rejected() {
    assert!(matches!(
        "atenet-router.ate-system.svc:https".parse::<RouterEndpoint>(),
        Err(RouterEndpointError::InvalidPort { .. })
    ));
}

#[test]
fn a_router_endpoint_round_trips_through_display() -> TestResult {
    let endpoint: RouterEndpoint = DEFAULT_ROUTER_ENDPOINT.parse()?;
    assert_eq!(endpoint.host(), "atenet-router.ate-system.svc");
    assert_eq!(endpoint.port(), 8444);
    assert_eq!(endpoint.to_string(), DEFAULT_ROUTER_ENDPOINT);
    Ok(())
}

#[test]
fn sandbox_classes_parse_from_their_flag_spellings() -> TestResult {
    assert_eq!("gvisor".parse::<SandboxClass>()?, SandboxClass::Gvisor);
    assert_eq!(" MicroVM ".parse::<SandboxClass>()?, SandboxClass::MicroVm);
    let error = "kata".parse::<SandboxClass>().err().ok_or("kata is not a class")?;
    assert_eq!(error.class(), "kata");
    Ok(())
}

#[test]
fn a_sandbox_class_displays_as_its_flag_spelling() {
    assert_eq!(SandboxClass::Gvisor.to_string(), "gvisor");
    assert_eq!(SandboxClass::MicroVm.to_string(), "microvm");
}

#[test]
fn each_sandbox_class_maps_to_the_tier_it_satisfies_and_back() {
    for class in [SandboxClass::Gvisor, SandboxClass::MicroVm] {
        assert_eq!(SandboxClass::for_tier(class.isolation_tier()), Some(class));
    }
    assert_eq!(SandboxClass::for_tier(IsolationTier::Container), None);
    assert_eq!(SandboxClass::for_tier(IsolationTier::FullVm), None);
}
