use trogon_sandbox_contract::{
    ArtifactType, PrewarmSpec, RetentionPolicy, SandboxImage, SandboxSpec, SandboxSpecError, SnapshotPolicy,
    SnapshotTag, SnapshotTagName, TenantId,
};

/// How a persistent spec captures and restores its tagged snapshot.
#[derive(Debug, Clone)]
pub(crate) struct PersistentSnapshotOptions {
    pub(crate) tag_name: SnapshotTagName,
    pub(crate) artifact_type: ArtifactType,
    pub(crate) target_repository: String,
    pub(crate) retention: Option<RetentionPolicy>,
    pub(crate) prewarm: Option<PrewarmSpec>,
}

/// Why a spec could not be made persistent.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PersistentSnapshotError {
    #[error("the rebuilt spec is invalid: {0}")]
    Spec(#[from] SandboxSpecError),
}

/// Makes `spec` resume from its most recent snapshot under the tag and capture a
/// new one under the same tag on release. Falls back to `base_image` when no
/// snapshot has been tagged yet.
pub(crate) fn with_persistence(
    spec: SandboxSpec,
    tenant: TenantId,
    base_image: Option<SandboxImage>,
    options: PersistentSnapshotOptions,
) -> Result<SandboxSpec, PersistentSnapshotError> {
    let mut parts = spec.into_parts();
    parts.image = SandboxImage::SnapshotTag {
        tag: SnapshotTag {
            tenant,
            name: options.tag_name.clone(),
        },
        artifact_type: options.artifact_type,
        create_from: base_image.map(Box::new),
    };
    parts.snapshot_out = Some(SnapshotPolicy {
        artifact_type: options.artifact_type,
        target_repository: options.target_repository,
        retention: options.retention,
        prewarm: options.prewarm,
        tag_name: Some(options.tag_name.as_str().to_owned()),
    });
    Ok(SandboxSpec::new(parts)?)
}
