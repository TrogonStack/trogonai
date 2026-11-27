use trogon_sandbox_contract::{DormancyCapability, EgressCapability};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn pause_is_a_freeze_that_keeps_memory_resident() {
    assert_eq!(PROFILE.lifecycle.dormancy, DormancyCapability::FREEZE);
}

#[test]
fn hardening_overrides_are_applied_by_the_docker_worker() {
    assert!(PROFILE.isolation.supports_hardening_overrides.is_supported());
}

#[test]
fn egress_is_enforced_through_the_sidecar_proxy() {
    assert_eq!(PROFILE.isolation.egress, EgressCapability::PROXIED);
}

#[test]
fn disk_is_not_claimed_without_an_operator_ceiling() -> TestResult {
    let r = resources(4, None);
    assert_eq!(r.disk_mib, None);
    assert_eq!(r.vcpu_millis, Some(ResourceRange::new(MIN_VCPU_MILLIS, 4000)?));
    Ok(())
}

#[test]
fn disk_is_claimed_when_the_operator_names_a_ceiling() -> TestResult {
    let r = resources(2, Some(20_480));
    assert_eq!(r.disk_mib, Some(ResourceRange::new(MIN_DISK_MIB, 20_480)?));
    assert!(profile_with_resources(r).resources.disk_mib.is_some());
    Ok(())
}

#[test]
fn ceilings_below_the_minimum_clamp_up_to_it() -> TestResult {
    let r = resources(0, Some(1));
    assert_eq!(
        r.vcpu_millis,
        Some(ResourceRange::new(MIN_VCPU_MILLIS, MIN_VCPU_MILLIS)?)
    );
    assert_eq!(r.disk_mib, Some(ResourceRange::new(MIN_DISK_MIB, MIN_DISK_MIB)?));
    Ok(())
}
