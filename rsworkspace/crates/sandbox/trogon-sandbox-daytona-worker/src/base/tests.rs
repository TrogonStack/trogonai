use super::*;

use trogon_sandbox_contract::{
    ArtifactType, NativeSnapshotId, NativeSnapshotRef, OciImageRef, SnapshotTag, SnapshotTagName, TenantId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn named(value: &str) -> SandboxImage {
    SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified(value),
        pull_secret: None,
    }
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
fn a_registry_reference_names_the_snapshot() -> TestResult {
    assert_eq!(
        Base::for_image(&named("agent:dev"), "default")?.into_name(),
        "agent:dev"
    );
    Ok(())
}

#[test]
fn an_empty_repository_takes_the_default_snapshot() -> TestResult {
    assert_eq!(Base::for_image(&named(""), "default")?.into_name(), "default");
    assert!(matches!(
        Base::for_image(&named(""), ""),
        Err(LeaseError::InvalidSpec { .. })
    ));
    Ok(())
}

#[test]
fn a_name_over_the_limit_is_refused() {
    let image = named(&"a".repeat(MAX_SNAPSHOT_NAME_LEN + 1));
    assert!(matches!(
        Base::for_image(&image, "default"),
        Err(LeaseError::InvalidSpec { .. })
    ));
}

#[test]
fn only_a_native_snapshot_of_this_provider_restores() -> TestResult {
    assert_eq!(
        Base::for_image(&native(ProviderKind::Daytona)?, "default")?.into_name(),
        "snap_1"
    );
    assert!(matches!(
        Base::for_image(&native(ProviderKind::Fly)?, "default"),
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
        Base::for_image(&tag, "default"),
        Err(LeaseError::SnapshotUnsupported { .. })
    ));
    Ok(())
}
