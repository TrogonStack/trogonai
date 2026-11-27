use super::*;

#[test]
fn the_profile_names_daytona_and_offers_native_snapshots_only() {
    assert_eq!(PROFILE.provider, ProviderKind::Daytona);
    assert_eq!(PROFILE.snapshot.supports_native_snapshots, CapabilitySupport::Supported);
    assert_eq!(PROFILE.snapshot.can_checkpoint_running, CapabilitySupport::Supported);
    assert_eq!(PROFILE.snapshot.can_restore, CapabilitySupport::Supported);
    assert_eq!(PROFILE.snapshot.can_snapshot, CapabilitySupport::Unsupported);
    assert!(PROFILE.snapshot.supported_artifact_types.is_empty());
    assert_eq!(PROFILE.lifecycle.supports_pause_resume, CapabilitySupport::Supported);
    assert!(PROFILE.resources.vcpu_millis.is_none());
}

#[test]
fn terminals_and_stdin_are_offered() {
    assert_eq!(PROFILE.isolation.supports_terminal, CapabilitySupport::Supported);
    assert_eq!(
        PROFILE.isolation.supports_interactive_stdin,
        CapabilitySupport::Supported
    );
}

#[test]
fn pause_is_a_stop_that_keeps_only_the_disk() {
    assert_eq!(PROFILE.lifecycle.dormancy, DormancyCapability::SUSPEND_DISK);
}

#[test]
fn unverified_provider_enforcement_cannot_satisfy_egress_requirements() {
    assert_eq!(PROFILE.isolation.egress_enforcement, EgressEnforcementLevel::None);
    for level in [EgressEnforcementLevel::Mandatory, EgressEnforcementLevel::BestEffort] {
        assert!(PROFILE.isolation.egress_enforcement < level);
    }
}

#[test]
fn unverified_egress_policies_are_not_advertised() {
    let egress = PROFILE.isolation.egress;
    assert_eq!(egress.deny_all, CapabilitySupport::Unsupported);
    assert_eq!(egress.cidr_allowlist, CapabilitySupport::Unsupported);
    assert_eq!(egress.host_allowlist, CapabilitySupport::Unsupported);
    assert_eq!(egress.proxy_mediated, CapabilitySupport::Unsupported);
}
