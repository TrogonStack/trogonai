mod support;

use support::*;
use trogon_sandbox_contract::{EgressCapability, IsolationTier};

#[test]
fn the_profile_advertises_what_modal_offers() -> TestResult {
    let env = Env::with_config(|config| config.max_vcpus = 2)?;
    let profile = env.provider.profile();

    let vcpus = profile.resources.vcpu_millis.ok_or("no vcpu range")?;
    assert_eq!((vcpus.min(), vcpus.max()), (125, 2000));

    assert_eq!(profile.isolation.egress, EgressCapability::DENY_ONLY);
    assert_eq!(profile.isolation.default_tier, IsolationTier::UserSpaceKernel);
    assert_eq!(profile.isolation.supported_tiers, vec![IsolationTier::UserSpaceKernel]);

    assert!(profile.isolation.supports_port_preview.is_supported());
    assert!(!profile.isolation.supports_expose_after_create.is_supported());
    assert!(!profile.lifecycle.supports_pause_resume.is_supported());
    assert!(!profile.volume.supports_fuse_mounts.is_supported());

    assert!(!profile.snapshot.can_snapshot.is_supported());
    assert!(profile.snapshot.can_checkpoint_running.is_supported());
    assert!(profile.snapshot.can_restore.is_supported());
    assert!(profile.snapshot.supports_native_snapshots.is_supported());
    assert!(profile.snapshot.supported_artifact_types.is_empty());
    Ok(())
}
