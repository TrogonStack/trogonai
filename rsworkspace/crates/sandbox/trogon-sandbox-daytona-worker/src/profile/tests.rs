use super::*;

#[test]
fn the_profile_names_daytona_and_offers_no_snapshots() {
    assert_eq!(PROFILE.provider, ProviderKind::Daytona);
    assert_eq!(
        PROFILE.snapshot.supports_native_snapshots,
        CapabilitySupport::Unsupported
    );
    assert_eq!(PROFILE.lifecycle.supports_pause_resume, CapabilitySupport::Supported);
    assert!(PROFILE.resources.vcpu_millis.is_none());
}

#[test]
fn egress_offers_deny_all_and_cidr_allow_lists_only() {
    assert_eq!(PROFILE.isolation.egress.deny_all, CapabilitySupport::Supported);
    assert_eq!(PROFILE.isolation.egress.cidr_allowlist, CapabilitySupport::Supported);
    assert_eq!(PROFILE.isolation.egress.host_allowlist, CapabilitySupport::Unsupported);
}
