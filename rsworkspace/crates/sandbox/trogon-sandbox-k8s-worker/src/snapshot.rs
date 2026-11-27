use trogon_sandbox_contract::{ArtifactType, OciArtifactRef, SnapshotPushTarget, SnapshotReceipt};
use trogon_sandbox_oci::{Annotations, Layer, OciClient, PulledArtifact, build_push_target};
use trogon_sandbox_providers::K8sMicroVmProvider;

use crate::SnapshotTransferError;
use crate::constants::ROOTFS_LAYER_MEDIA_TYPE;

pub fn artifact_layers_bytes(layers: &[Layer]) -> Vec<u8> {
    layers.iter().flat_map(|layer| layer.data.clone()).collect()
}

pub fn pulled_rootfs_bytes(pulled: &PulledArtifact) -> Vec<u8> {
    artifact_layers_bytes(&pulled.layers)
}

pub fn tar_directory(path: &std::path::Path) -> Result<Vec<u8>, SnapshotTransferError> {
    let mut builder = tar::Builder::new(Vec::new());
    if path.is_dir() {
        builder.append_dir_all(".", path).map_err(SnapshotTransferError::Tar)?;
    }
    builder.finish().map_err(SnapshotTransferError::Tar)?;
    builder.into_inner().map_err(SnapshotTransferError::Tar)
}

pub async fn export_and_push_rootfs(
    k8s: &K8sMicroVmProvider,
    oci: &OciClient,
    job_name: &str,
    push_target: &SnapshotPushTarget,
) -> Result<SnapshotReceipt, SnapshotTransferError> {
    let tar_bytes = k8s
        .export_workspace_tar(job_name)
        .await
        .map_err(SnapshotTransferError::Export)?;

    push_rootfs_tar(oci, push_target, &tar_bytes).await
}

pub async fn export_and_push_rootfs_from_pod(
    k8s: &K8sMicroVmProvider,
    oci: &OciClient,
    pod_name: &str,
    push_target: &SnapshotPushTarget,
) -> Result<SnapshotReceipt, SnapshotTransferError> {
    let tar_bytes = k8s
        .export_workspace_tar_from_pod(pod_name)
        .await
        .map_err(SnapshotTransferError::Export)?;

    push_rootfs_tar(oci, push_target, &tar_bytes).await
}

pub async fn push_rootfs_tar(
    oci: &OciClient,
    push_target: &SnapshotPushTarget,
    tar_bytes: &[u8],
) -> Result<SnapshotReceipt, SnapshotTransferError> {
    let layers = vec![rootfs_layer(tar_bytes)];
    let target = build_push_target(
        &push_target.registry,
        &push_target.repository,
        push_target.artifact_type,
        &layers,
        &Annotations::default(),
    )
    .map_err(SnapshotTransferError::BuildPushTarget)?;

    oci.push_artifact(&target, layers, Annotations::default())
        .await
        .map_err(SnapshotTransferError::Push)?;

    Ok(SnapshotReceipt {
        digest: target.digest,
        bytes: tar_bytes.len() as u64,
        artifact_type: push_target.artifact_type,
    })
}

pub async fn pull_rootfs(oci: &OciClient, reference: &OciArtifactRef) -> Result<Vec<u8>, SnapshotTransferError> {
    if reference.artifact_type != ArtifactType::SandboxRootfsV1 {
        return Err(SnapshotTransferError::UnsupportedArtifactType {
            found: reference.artifact_type,
        });
    }
    let pulled = oci
        .pull_artifact(reference)
        .await
        .map_err(SnapshotTransferError::Pull)?;
    Ok(pulled_rootfs_bytes(&pulled))
}

fn rootfs_layer(tar_bytes: &[u8]) -> Layer {
    Layer {
        data: tar_bytes.to_vec(),
        media_type: ROOTFS_LAYER_MEDIA_TYPE.to_string(),
        annotations: Annotations::default(),
    }
}
