use trogon_sandbox_contract::{
    ArtifactType, Digest, DriveId, DriveMount, DriveMountMode, DriveName, HostMountAccess, IsolationTier, MountAccess,
    MountPath, NativeSnapshotId, NativeSnapshotRef, OciArtifactRef, RemoteMount, RemoteMountSource, S3BucketName,
    SandboxImage, SandboxSpec, SnapshotTag, SnapshotTagName, WorkspaceSource,
};

use super::super::tests_support::{TestResult, lease_request, registry_image, spec, spec_parts};
use super::*;

fn refused(error: SubstrateError, needle: &str) -> TestResult {
    match error {
        SubstrateError::InvalidSpec(message) => {
            assert!(message.contains(needle), "{message:?} does not mention {needle:?}");
            Ok(())
        }
        other => Err(format!("expected InvalidSpec, got {other:?}").into()),
    }
}

fn artifact() -> Result<OciArtifactRef, Box<dyn Error>> {
    Ok(OciArtifactRef {
        registry: "registry.example".to_string(),
        repository: "snapshots/app".to_string(),
        digest: Digest::new(format!("sha256:{}", "a".repeat(64)))?,
        artifact_type: ArtifactType::HyperlightSnapshotV1,
    })
}

#[test]
fn a_spec_this_provider_can_honor_is_accepted() -> TestResult {
    assert!(validate_spec(&spec()?).is_ok());
    Ok(())
}

#[test]
fn explicit_cpu_sizing_is_refused() -> TestResult {
    let mut parts = spec_parts()?;
    parts.vcpu_millis = Some(4000);
    let error = validate_spec(&SandboxSpec::new(parts)?).err().ok_or("accepted")?;
    refused(error, "vcpu_millis")
}

#[test]
fn an_environment_is_refused() -> TestResult {
    let mut parts = spec_parts()?;
    parts.env.insert("A".to_string(), "1".to_string());
    let error = validate_spec(&SandboxSpec::new(parts)?).err().ok_or("accepted")?;
    refused(error, "spec.env")
}

#[test]
fn remote_mounts_are_refused() -> TestResult {
    let mut parts = spec_parts()?;
    parts.mounts.push(RemoteMount {
        source: RemoteMountSource::S3 {
            bucket: S3BucketName::new("bucket")?,
            prefix: "p".to_string(),
        },
        mount_path: MountPath::new("/mnt")?,
        access: MountAccess::ReadWrite,
    });
    let error = validate_spec(&SandboxSpec::new(parts)?).err().ok_or("accepted")?;
    refused(error, "remote mounts")
}

#[test]
fn persistent_drives_are_refused() -> TestResult {
    let mut parts = spec_parts()?;
    parts.drives.push(DriveMount {
        drive_id: DriveId {
            tenant: tenant_a()?,
            name: DriveName::new("d")?,
        },
        mount_path: MountPath::new("/data")?,
        mode: DriveMountMode::ReadWrite,
    });
    let error = validate_spec(&SandboxSpec::new(parts)?).err().ok_or("accepted")?;
    refused(error, "persistent drives")
}

#[test]
fn privileged_access_is_refused() -> TestResult {
    let mut parts = spec_parts()?;
    parts.privileged = true.into();
    let error = validate_spec(&SandboxSpec::new(parts)?).err().ok_or("accepted")?;
    refused(error, "privileged")
}

#[test]
fn a_user_override_is_refused() -> TestResult {
    let mut parts = spec_parts()?;
    parts.user = Some("1000".to_string());
    let error = validate_spec(&SandboxSpec::new(parts)?).err().ok_or("accepted")?;
    refused(error, "user overrides")
}

#[test]
fn two_exposed_ports_are_refused() -> TestResult {
    let mut parts = spec_parts()?;
    parts.exposed_ports = vec![Port::new(80)?, Port::new(443)?];
    let error = validate_spec(&SandboxSpec::new(parts)?).err().ok_or("accepted")?;
    refused(error, "at most one port")
}

#[test]
fn a_host_mount_workspace_is_refused() -> TestResult {
    let mut parts = spec_parts()?;
    parts.workspace = WorkspaceSource::HostMount {
        path: "/srv".to_string(),
        access: HostMountAccess::ReadWrite,
    };
    let error = validate_spec(&SandboxSpec::new(parts)?).err().ok_or("accepted")?;
    refused(error, "host_mount")
}

#[test]
fn a_single_exposed_port_is_allowed() -> TestResult {
    let mut parts = spec_parts()?;
    parts.exposed_ports = vec![Port::new(8080)?];
    assert!(validate_spec(&SandboxSpec::new(parts)?).is_ok());
    Ok(())
}

#[test]
fn a_registry_image_backs_an_actor_template() -> TestResult {
    assert_eq!(
        image_reference(&registry_image())?,
        super::super::tests_support::app_image()
    );
    Ok(())
}

#[test]
fn every_other_kind_of_image_is_refused_by_name() -> TestResult {
    let cases = [
        (
            SandboxImage::Snapshot {
                reference: artifact()?,
                parent: None,
            },
            "a snapshot artifact",
        ),
        (
            SandboxImage::SnapshotTag {
                tag: SnapshotTag {
                    tenant: tenant_a()?,
                    name: SnapshotTagName::new("latest")?,
                },
                artifact_type: ArtifactType::HyperlightSnapshotV1,
                create_from: None,
            },
            "a snapshot tag",
        ),
        (
            SandboxImage::HyperlightModule { reference: artifact()? },
            "a hyperlight module",
        ),
        (
            SandboxImage::NativeSnapshot {
                reference: NativeSnapshotRef {
                    provider: ProviderKind::Substrate,
                    id: NativeSnapshotId::new("snap-1")?,
                    expires_at: None,
                },
            },
            "a native provider snapshot",
        ),
    ];
    for (image, kind) in cases {
        refused(image_reference(&image).err().ok_or("accepted")?, kind)?;
    }
    Ok(())
}

#[test]
fn the_default_tier_is_the_configured_class() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    assert_eq!(
        provider.sandbox_class_for(&lease_request(None, true)?)?,
        SandboxClass::Gvisor
    );
    Ok(())
}

#[test]
fn a_requested_tier_this_provider_serves_selects_its_sandbox_class() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let mut request = lease_request(None, true)?;
    request.isolation_tier = Some(IsolationTier::MicroVm);
    assert_eq!(provider.sandbox_class_for(&request)?, SandboxClass::MicroVm);
    Ok(())
}

#[test]
fn an_unsupported_isolation_tier_is_refused() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let mut request = lease_request(None, true)?;
    request.isolation_tier = Some(IsolationTier::FullVm);
    refused(
        provider.sandbox_class_for(&request).err().ok_or("accepted")?,
        "not available",
    )
}

#[test]
fn the_provider_advertises_only_the_tiers_it_serves() {
    assert!(supports_tier(IsolationTier::MicroVm));
    assert!(!supports_tier(IsolationTier::FullVm));
}

#[test]
fn the_sandbox_id_names_the_actor_within_its_atespace() -> TestResult {
    let lease = anonymous(&LeaseId::new("01ARZ"), true, None)?;
    assert_eq!(lease.sandbox_id(), "tenant-a/sbx-01arz");
    Ok(())
}

#[test]
fn an_actor_without_status_is_unspecified() {
    assert_eq!(actor_state(&proto::Actor::default()), proto::ActorState::Unspecified);
}
