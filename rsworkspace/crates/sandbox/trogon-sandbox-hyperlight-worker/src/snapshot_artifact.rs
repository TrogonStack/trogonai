use trogon_sandbox_oci::{ArtifactType, Layer, OciArtifactRef, OciClient, build_push_target};

use crate::SnapshotTransportError;
pub use crate::constants::HYPERLIGHT_SNAPSHOT_MEDIA_TYPE;

/// The artifact layer that carries a packed snapshot layout.
pub fn snapshot_layer(blob: Vec<u8>) -> Layer {
    Layer {
        data: blob,
        media_type: HYPERLIGHT_SNAPSHOT_MEDIA_TYPE.to_string(),
        annotations: Default::default(),
    }
}

/// Pushes `blob` as a Hyperlight snapshot artifact and returns its reference and size.
pub async fn push_snapshot_artifact(
    client: &OciClient,
    registry: &str,
    repository: &str,
    blob: Vec<u8>,
) -> Result<(OciArtifactRef, u64), SnapshotTransportError> {
    let bytes = blob.len() as u64;
    let layers = vec![snapshot_layer(blob)];
    let target = build_push_target(
        registry,
        repository,
        ArtifactType::HyperlightSnapshotV1,
        &layers,
        &Default::default(),
    )
    .map_err(SnapshotTransportError::Target)?;
    client
        .push_artifact(&target, layers, Default::default())
        .await
        .map_err(SnapshotTransportError::Push)?;
    Ok((target, bytes))
}

/// Pulls the snapshot artifact `reference` and returns its first layer.
pub async fn pull_snapshot_blob(
    client: &OciClient,
    reference: &OciArtifactRef,
) -> Result<Vec<u8>, SnapshotTransportError> {
    let pulled = client
        .pull_artifact(reference)
        .await
        .map_err(|source| SnapshotTransportError::Pull {
            reference: reference.allowlist_string(),
            source,
        })?;
    pulled
        .layers
        .into_iter()
        .next()
        .map(|layer| layer.data)
        .ok_or(SnapshotTransportError::NoLayers)
}
