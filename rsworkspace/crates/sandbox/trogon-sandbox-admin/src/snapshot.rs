use async_nats::jetstream;
use trogon_sandbox_broker::snapshot_capture_status::{NatsKvSnapshotCaptureStatusStore, SnapshotCaptureStatusStore};
use trogon_sandbox_broker::snapshot_location::locate_snapshot_repository;
use trogon_sandbox_contract::{ArtifactType, Digest, LeaseId, OciArtifactRef, SnapshotCaptureStatus, TenantId};
use trogon_sandbox_oci::{OciClient, OciClientConfig, RegistryHost};

use crate::{AdminError, Console};

/// Where to delete a snapshot artifact from.
pub struct SnapshotDeleteRequest {
    pub registry: RegistryHost,
    pub insecure_registry: bool,
    pub tenant: TenantId,
    pub digest: Digest,
    pub artifact_type: ArtifactType,
}

/// Deletes a snapshot artifact by digest straight from the registry.
pub async fn delete(request: SnapshotDeleteRequest, console: &mut Console<'_>) -> Result<(), AdminError> {
    let SnapshotDeleteRequest {
        registry,
        insecure_registry,
        tenant,
        digest,
        artifact_type,
    } = request;
    let registry_name = registry.to_string();
    let client = OciClient::new(OciClientConfig {
        insecure_registries: if insecure_registry { vec![registry] } else { vec![] },
        ..Default::default()
    });

    // Same lookup the broker uses to evict, so an operator deleting by digest lands on whichever
    // of the tenant's repositories holds the artifact.
    let repository = locate_snapshot_repository(Some(&client), &registry_name, &tenant, &digest, artifact_type).await;
    let target = OciArtifactRef {
        registry: registry_name,
        repository,
        digest,
        artifact_type,
    };

    client
        .delete_artifact(&target)
        .await
        .map_err(AdminError::step("delete snapshot artifact"))?;
    console.err(format_args!("deleted {}", target.allowlist_string()))
}

/// Reports whether a lease's snapshot capture succeeded or failed.
pub async fn status(js: jetstream::Context, lease_id: &LeaseId, console: &mut Console<'_>) -> Result<(), AdminError> {
    let store = NatsKvSnapshotCaptureStatusStore::new(js)
        .await
        .map_err(AdminError::step("open snapshot capture status store"))?;
    match store
        .get(lease_id)
        .await
        .map_err(AdminError::step("read snapshot capture status"))?
    {
        Some(SnapshotCaptureStatus::Created { digest, artifact_type }) => {
            console.out(format_args!("created {digest} {artifact_type:?}"))
        }
        Some(SnapshotCaptureStatus::Failed { reason }) => console.out(format_args!("failed {reason}")),
        None => Err(AdminError::NoCaptureStatus {
            lease_id: lease_id.to_string(),
        }),
    }
}
