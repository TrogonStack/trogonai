use super::*;

use crate::core::persistent_snapshot::{PersistentSnapshotOptions, with_persistence};
use trogon_sandbox_contract::{ArtifactType, SnapshotTag, SnapshotTagName};

fn options() -> anyhow::Result<PersistentSnapshotOptions> {
    Ok(PersistentSnapshotOptions {
        tag_name: SnapshotTagName::new("saved")?,
        artifact_type: ArtifactType::SandboxRootfsV1,
        target_repository: "snapshots/test-tenant".into(),
        retention: None,
        prewarm: None,
    })
}

#[test]
fn a_persistent_spec_without_a_base_image_has_nothing_to_create_from() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let spec = with_persistence(make_spec()?, tenant.clone(), None, options()?)?;
    let SandboxImage::SnapshotTag {
        tag,
        artifact_type,
        create_from,
    } = spec.image()
    else {
        return Err("expected a snapshot tag image".into());
    };
    assert_eq!(
        tag,
        &SnapshotTag {
            tenant,
            name: SnapshotTagName::new("saved")?
        }
    );
    assert_eq!(*artifact_type, ArtifactType::SandboxRootfsV1);
    assert!(create_from.is_none());
    Ok(())
}

#[test]
fn a_persistent_spec_with_a_base_image_falls_back_to_it() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let base = SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified("ubuntu:24.04"),
        pull_secret: None,
    };
    let spec = with_persistence(make_spec()?, tenant, Some(base.clone()), options()?)?;
    let SandboxImage::SnapshotTag { create_from, .. } = spec.image() else {
        return Err("expected a snapshot tag image".into());
    };
    assert_eq!(create_from.as_deref(), Some(&base));
    Ok(())
}
