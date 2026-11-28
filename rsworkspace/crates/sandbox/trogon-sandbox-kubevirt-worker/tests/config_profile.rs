use std::error::Error;

use trogon_sandbox_contract::{ArtifactType, CapabilitySupport, IsolationTier, ProviderKind, ResourceRange, WorkerId};
use trogon_sandbox_kubevirt_worker::{KubeVirtConfig, WarmPoolConfig, profile};
use trogon_sandbox_oci::OciClientConfig;

type TestResult = Result<(), Box<dyn Error>>;

fn config(disk_size_gi: u32) -> Result<KubeVirtConfig, Box<dyn Error>> {
    Ok(KubeVirtConfig {
        namespace: "sandboxes".into(),
        default_image: "registry.example/base:1".into(),
        storage_class: None,
        snapshot_registry: "registry.example".into(),
        worker_id: WorkerId::new("kubevirt-0")?,
        disk_size_gi,
        bootstrap_ssh_pubkey: None,
        s3_secret_name: None,
        restore_bucket_prefix: "restore".into(),
        oci: OciClientConfig::default(),
        cosign_verifier: None,
        volume: None,
        warm_pool: WarmPoolConfig::default(),
    })
}

#[test]
fn the_root_disk_is_never_smaller_than_one_gibibyte() -> TestResult {
    assert_eq!(config(0)?.disk_size_quantity(), "1Gi");
    assert_eq!(config(1)?.disk_size_quantity(), "1Gi");
    assert_eq!(config(40)?.disk_size_quantity(), "40Gi");
    let copy = config(2)?.clone();
    assert!(format!("{copy:?}").contains("sandboxes"));
    Ok(())
}

#[test]
fn the_profile_describes_a_full_vm_provider() -> TestResult {
    let profile = profile();
    assert_eq!(profile.provider, ProviderKind::KubeVirt);
    assert!(matches!(profile.isolation.default_tier, IsolationTier::FullVm));
    assert_eq!(profile.resources.vcpu_millis, Some(ResourceRange::new(100, 8000)?));
    assert_eq!(profile.lifecycle.max_warm_pool_size, Some(4));
    Ok(())
}

#[test]
fn the_profile_offers_online_snapshots_of_kubevirt_disks() {
    let profile = profile();
    let snapshot = &profile.snapshot;
    assert_eq!(snapshot.can_snapshot, CapabilitySupport::Supported);
    assert_eq!(snapshot.can_restore, CapabilitySupport::Supported);
    assert_eq!(snapshot.can_checkpoint_running, CapabilitySupport::Supported);
    assert_eq!(snapshot.supported_artifact_types, vec![ArtifactType::KubevirtDiskV1]);
    assert_eq!(profile.volume.supports_s3_backend, CapabilitySupport::Supported);
    assert_eq!(profile.lifecycle.supports_warm_pool, CapabilitySupport::Supported);
}
