use super::SnapshotManifestProbe;
use trogon_sandbox_contract::{ArtifactType, Digest, OciArtifactRef, TenantId};
use trogon_sandbox_protocol::{legacy_snapshot_repository_for_tenant, snapshot_repository_for_tenant};

/// Repository that holds `digest` for `tenant`: the current repository
/// unless only the legacy one has the manifest. Without a probe, or when
/// neither repository answers, the current repository is returned.
pub async fn locate_snapshot_repository(
    probe: Option<&dyn SnapshotManifestProbe>,
    registry: &str,
    tenant: &TenantId,
    digest: &Digest,
    artifact_type: ArtifactType,
) -> String {
    let current = snapshot_repository_for_tenant(tenant);
    let legacy = legacy_snapshot_repository_for_tenant(tenant);
    let Some(probe) = probe else {
        return current;
    };
    if legacy == current {
        return current;
    }
    let artifact = |repository: &str| OciArtifactRef {
        registry: registry.to_string(),
        repository: repository.to_string(),
        digest: digest.clone(),
        artifact_type,
    };
    if probe.exists(&artifact(&current)).await {
        return current;
    }
    if probe.exists(&artifact(&legacy)).await {
        return legacy;
    }
    current
}
