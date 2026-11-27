use std::io::Cursor;
use std::path::Path;

use trogon_sandbox_volumes::{VolumeClient, VolumeClientConfig, VolumeMode, VolumePrefix, build_changed_manifest};

use crate::{DaytonaApi, S3ExportError, S3ExportTarget, WorkspaceExportSummary};

/// Unpacks an in-memory tar archive into `dest_dir` and pushes every file under `prefix`.
///
/// It never reaches a sandbox, so a local-directory backend can stand in for S3.
pub async fn unpack_and_push_tar(
    tar_bytes: &[u8],
    dest_dir: &Path,
    config: VolumeClientConfig,
    prefix: &VolumePrefix,
) -> Result<WorkspaceExportSummary, S3ExportError> {
    tokio::fs::create_dir_all(dest_dir)
        .await
        .map_err(S3ExportError::Scratch)?;

    let tar_bytes = tar_bytes.to_vec();
    let unpack_dir = dest_dir.to_path_buf();
    tokio::task::spawn_blocking(move || tar::Archive::new(Cursor::new(tar_bytes)).unpack(&unpack_dir))
        .await
        .map_err(S3ExportError::UnpackTask)?
        .map_err(S3ExportError::Unpack)?;

    let client = VolumeClient::with_mode(config, VolumeMode::ReadWrite)?;
    let manifest = build_changed_manifest(dest_dir, None)?;
    let summary = client.push_changed(prefix, dest_dir, manifest).await?;
    Ok(WorkspaceExportSummary {
        objects: summary.objects,
        bytes: summary.bytes,
    })
}

/// Archives `workspace_path` inside the sandbox, downloads it, unpacks it locally and pushes it to `target`.
pub async fn export_workspace_to_s3(
    api: &DaytonaApi,
    sandbox_id: &str,
    workspace_path: &str,
    target: &S3ExportTarget,
) -> Result<WorkspaceExportSummary, S3ExportError> {
    let archive = api.archive_directory(sandbox_id, workspace_path).await?;
    let scratch = tempfile::tempdir().map_err(S3ExportError::Scratch)?;
    unpack_and_push_tar(&archive, scratch.path(), target.client_config(), &target.prefix).await
}

#[cfg(test)]
mod tests;
