use trogon_sandbox_contract::{CapabilitySupport, DormancyCapability, IsolationTier, ProviderKind, SdkFeature};

use super::PROFILE;

#[test]
fn the_profile_names_e2b_and_a_microvm() {
    assert_eq!(PROFILE.provider, ProviderKind::E2b);
    assert_eq!(PROFILE.isolation.default_tier, IsolationTier::MicroVm);
}

#[test]
fn egress_takes_hosts_and_cidr_blocks() {
    let egress = &PROFILE.isolation.egress;
    assert_eq!(egress.deny_all, CapabilitySupport::Supported);
    assert_eq!(egress.host_allowlist, CapabilitySupport::Supported);
    assert_eq!(egress.cidr_allowlist, CapabilitySupport::Supported);
    assert_eq!(egress.proxy_mediated, CapabilitySupport::Unsupported);
}

#[test]
fn pause_is_a_memory_suspend_not_a_freeze() {
    assert_eq!(PROFILE.lifecycle.dormancy, DormancyCapability::SUSPEND_MEMORY);
    assert_eq!(PROFILE.lifecycle.supports_pause_resume, CapabilitySupport::Supported);
    assert_eq!(PROFILE.snapshot.can_snapshot, CapabilitySupport::Unsupported);
}

#[test]
fn file_watch_and_change_tracking_are_both_declared() {
    assert_eq!(PROFILE.isolation.supports_file_watch, CapabilitySupport::Supported);
    assert_eq!(PROFILE.isolation.supports_change_tracking, CapabilitySupport::Supported);
}

#[test]
fn exact_command_cancellation_is_earned_without_claiming_graceful_stop() {
    assert_eq!(
        PROFILE.isolation.supports_command_cancellation,
        CapabilitySupport::Supported
    );
    assert!(PROFILE.sdk_features().contains(&SdkFeature::CommandCancellation));
    assert_eq!(PROFILE.isolation.supports_graceful_stop, CapabilitySupport::Unsupported);
}
