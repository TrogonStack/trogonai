use trogon_sandbox_contract::{CapabilitySupport, EgressEnforcementLevel, SuspendLevel};
use trogon_sandbox_space_worker::{MaxLeases, capability_profile};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn only_the_dormancy_the_worker_wires_is_advertised() -> TestResult {
    let profile = capability_profile(MaxLeases::parse(4)?);
    assert_eq!(profile.lifecycle.supports_pause_resume, CapabilitySupport::Supported);
    assert_eq!(profile.lifecycle.dormancy.freeze, CapabilitySupport::Supported);
    assert_eq!(profile.lifecycle.dormancy.suspend, Some(SuspendLevel::Memory));
    assert_eq!(profile.snapshot.can_snapshot, CapabilitySupport::Unsupported);
    assert_eq!(profile.snapshot.can_restore, CapabilitySupport::Unsupported);
    Ok(())
}

#[test]
fn buffered_output_and_guest_command_files_are_not_overstated() -> TestResult {
    let isolation = capability_profile(MaxLeases::parse(4)?).isolation;
    assert_eq!(isolation.supports_output_streaming, CapabilitySupport::Unsupported);
    assert_eq!(isolation.supports_file_ops, CapabilitySupport::Unsupported);
    assert_eq!(isolation.supports_port_preview, CapabilitySupport::Unsupported);
    Ok(())
}

#[test]
fn no_network_policy_is_claimed_and_no_port_is_exposed() -> TestResult {
    let profile = capability_profile(MaxLeases::parse(4)?);
    assert_eq!(profile.isolation.egress_enforcement, EgressEnforcementLevel::None);
    assert_eq!(profile.ingress.max_exposed_ports, Some(0));
    Ok(())
}

#[test]
fn the_lease_ceiling_is_the_concurrency_limit() -> TestResult {
    let profile = capability_profile(MaxLeases::parse(7)?);
    assert_eq!(profile.performance.max_concurrent_sandboxes, Some(7));
    Ok(())
}
