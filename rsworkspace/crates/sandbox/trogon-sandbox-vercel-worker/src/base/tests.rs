use super::*;

use trogon_sandbox_contract::{
    ArtifactType, NativeSnapshotId, NativeSnapshotRef, OciImageRef, SnapshotTag, SnapshotTagName, TenantId,
};

fn registry(reference: OciImageRef) -> SandboxImage {
    SandboxImage::Registry {
        reference,
        pull_secret: None,
    }
}

fn named(value: &str) -> SandboxImage {
    registry(OciImageRef::parse_unqualified(value))
}

fn native(provider: ProviderKind) -> Result<SandboxImage, Box<dyn std::error::Error>> {
    Ok(SandboxImage::NativeSnapshot {
        reference: NativeSnapshotRef {
            provider,
            id: NativeSnapshotId::new("snap_1")?,
            expires_at: None,
        },
    })
}

#[test]
fn a_known_untagged_runtime_name_keeps_runtime_semantics() {
    let base = Base::for_image(&named("node24"), "node22");
    assert_eq!(base, Ok(Base::Runtime("node24".to_owned())));
}

#[test]
fn a_tagged_or_qualified_reference_is_an_image() {
    assert_eq!(
        Base::for_image(&named("node24:stable"), "node22"),
        Ok(Base::Image("node24:stable".to_owned()))
    );
    assert_eq!(
        Base::for_image(&named("vercel/sandbox/node:24"), "node22"),
        Ok(Base::Image("vercel/sandbox/node:24".to_owned()))
    );
}

#[test]
fn an_empty_repository_takes_the_default_runtime() {
    let image = registry(OciImageRef::parse_unqualified(""));
    assert_eq!(
        Base::for_image(&image, "python3.13"),
        Ok(Base::Runtime("python3.13".to_owned()))
    );
}

#[test]
fn an_empty_repository_with_an_unknown_default_runtime_is_refused() {
    let image = registry(OciImageRef::parse_unqualified(""));
    assert!(matches!(
        Base::for_image(&image, "ruby3"),
        Err(LeaseError::InvalidSpec { .. })
    ));
}

#[test]
fn an_image_reference_over_the_vendor_limit_is_refused() {
    let image = named(&"a".repeat(MAX_IMAGE_LEN + 1));
    assert!(matches!(
        Base::for_image(&image, "node24"),
        Err(LeaseError::InvalidSpec { .. })
    ));
}

#[test]
fn only_a_native_snapshot_of_this_provider_restores() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        Base::for_image(&native(ProviderKind::Vercel)?, "node24"),
        Ok(Base::Snapshot("snap_1".to_owned()))
    );
    assert!(matches!(
        Base::for_image(&native(ProviderKind::Fly)?, "node24"),
        Err(LeaseError::InvalidSpec { .. })
    ));
    let tag = SandboxImage::SnapshotTag {
        tag: SnapshotTag {
            tenant: TenantId::new("t")?,
            name: SnapshotTagName::new("prod")?,
        },
        artifact_type: ArtifactType::SandboxRootfsV1,
        create_from: None,
    };
    assert!(matches!(
        Base::for_image(&tag, "node24"),
        Err(LeaseError::SnapshotUnsupported { .. })
    ));
    Ok(())
}

#[test]
fn each_base_exposes_only_its_own_part() {
    let runtime = Base::Runtime("node24".to_owned());
    let image = Base::Image("img".to_owned());
    let snapshot = Base::Snapshot("snap_1".to_owned());
    assert_eq!(runtime.runtime().as_deref(), Some("node24"));
    assert!(runtime.image().is_none() && runtime.source().is_none());
    assert_eq!(image.image().as_deref(), Some("img"));
    assert!(image.runtime().is_none() && image.source().is_none());
    assert_eq!(
        snapshot.source(),
        Some(serde_json::json!({"type": "snapshot", "snapshotId": "snap_1"}))
    );
    assert!(snapshot.runtime().is_none() && snapshot.image().is_none());
}
