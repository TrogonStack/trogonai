use super::*;
use crate::config::SandboxClass;

#[test]
fn the_profile_claims_no_capability_the_control_plane_cannot_back() {
    assert!(!PROFILE.snapshot.can_snapshot.is_supported());
    assert!(!PROFILE.snapshot.can_checkpoint_running.is_supported());
    assert!(!PROFILE.snapshot.can_restore.is_supported());
    assert!(PROFILE.snapshot.supported_artifact_types.is_empty());
    assert!(!PROFILE.isolation.supports_privileged.is_supported());
    assert!(PROFILE.isolation.supports_sandbox_users.is_supported());
    assert!(!PROFILE.volume.supports_fuse_mounts.is_supported());
    assert!(!PROFILE.volume.supports_s3_backend.is_supported());
    assert_eq!(PROFILE.isolation.egress_enforcement, EgressEnforcementLevel::None);
}

#[test]
fn the_profile_declares_the_one_port_ingress_limit() {
    assert_eq!(PROFILE.ingress.max_exposed_ports, Some(1));
    assert!(PROFILE.ingress.allows(1));
    assert!(!PROFILE.ingress.allows(2));
}

#[test]
fn the_default_tier_is_one_of_the_supported_tiers() {
    assert!(
        PROFILE
            .isolation
            .supported_tiers
            .contains(&PROFILE.isolation.default_tier)
    );
}

#[test]
fn every_supported_tier_maps_to_a_sandbox_class_this_worker_can_run() {
    for tier in &PROFILE.isolation.supported_tiers {
        assert!(
            SandboxClass::for_tier(*tier).is_some(),
            "advertised tier {tier:?} has no sandbox class"
        );
    }
}
