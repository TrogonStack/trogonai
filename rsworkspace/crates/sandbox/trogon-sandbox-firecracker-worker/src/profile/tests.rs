use super::*;
use crate::config::{BaseRootfs, FirecrackerBinary, FirecrackerConfig, KernelImage, StateDir};

#[test]
fn only_a_networked_worker_claims_allow_lists() {
    assert!(
        !for_worker(false, false)
            .isolation
            .supports_domain_allowlist
            .is_supported()
    );
    assert!(
        for_worker(true, false)
            .isolation
            .supports_domain_allowlist
            .is_supported()
    );
    for networked in [false, true] {
        let isolation = &for_worker(networked, false).isolation;
        assert_eq!(isolation.egress_enforcement, EgressEnforcementLevel::Mandatory);
        assert!(!isolation.supports_tls_intercept.is_supported());
        assert!(!isolation.supports_secret_substitution.is_supported());
    }
}

fn worker_config() -> FirecrackerConfig {
    FirecrackerConfig::new(
        FirecrackerBinary::new("firecracker"),
        KernelImage::new("/k"),
        BaseRootfs::new("/r"),
        StateDir::new("/s"),
    )
}

fn advertised(config: &FirecrackerConfig) -> ProviderCapabilityProfile {
    let mut profile = PROFILE.clone();
    with_network(&mut profile, config);
    profile
}

#[test]
fn interception_and_injection_follow_the_networking_and_broker_configuration() -> Result<(), ipnet::AddrParseError> {
    let mut config = worker_config();
    let none = advertised(&config).isolation;
    assert!(!none.supports_tls_intercept.is_supported() && !none.supports_secret_substitution.is_supported());

    config.credential_broker_nats_url = Some("nats://broker".into());
    let broker_only = advertised(&config).isolation;
    assert!(
        !broker_only.supports_tls_intercept.is_supported() && !broker_only.supports_secret_substitution.is_supported()
    );

    config.network = Some(crate::network::NetworkConfig {
        host_links: "10.231.0.0/24".parse()?,
    });
    let injecting = advertised(&config).isolation;
    assert!(injecting.supports_tls_intercept.is_supported() && injecting.supports_secret_substitution.is_supported());
    assert!(injecting.supports_domain_allowlist.is_supported());

    config.credential_broker_nats_url = None;
    let intercepting = advertised(&config).isolation;
    assert!(
        intercepting.supports_tls_intercept.is_supported() && !intercepting.supports_secret_substitution.is_supported()
    );
    assert!(!intercepting.supports_secret_bindings.is_supported());
    Ok(())
}

#[test]
fn the_profile_describes_a_networkless_microvm() {
    assert_eq!(PROFILE.provider, ProviderKind::Firecracker);
    assert_eq!(PROFILE.isolation.default_tier, IsolationTier::MicroVm);
    assert_eq!(PROFILE.isolation.egress_enforcement, EgressEnforcementLevel::Mandatory);
    assert!(PROFILE.isolation.supports_privileged.is_supported());
}

#[test]
fn no_unbuilt_capability_is_claimed() {
    for snapshots in [false, true] {
        let profile = for_worker(false, snapshots);
        assert!(!profile.lifecycle.supports_warm_pool.is_supported());
        assert!(!profile.volume.supports_fuse_mounts.is_supported());
        assert!(!profile.snapshot.layered_deltas.is_supported());
    }
}

#[test]
fn only_a_worker_that_keeps_snapshots_claims_them() {
    let without = for_worker(false, false).snapshot;
    assert!(
        !without.can_snapshot.is_supported()
            && !without.can_checkpoint_running.is_supported()
            && !without.can_restore.is_supported()
    );
    assert!(without.supported_artifact_types.is_empty());

    let with = for_worker(false, true);
    assert!(with.snapshot.can_checkpoint_running.is_supported() && with.snapshot.can_restore.is_supported());
    assert_eq!(
        with.snapshot.supported_artifact_types,
        vec![ArtifactType::FirecrackerSnapshotV1]
    );
    assert!(with.performance.cold_restore_p50_ms > 0);
}

#[test]
fn a_profile_advertises_no_environment_digest_until_the_worker_computes_one() {
    assert_eq!(for_worker(false, true).placement.env_digest, None);
}

#[test]
fn declared_tiers_agree_with_the_wire_contract() {
    assert!(
        ProviderKind::Firecracker
            .isolation_tiers()
            .contains(&PROFILE.isolation.default_tier)
    );
}
