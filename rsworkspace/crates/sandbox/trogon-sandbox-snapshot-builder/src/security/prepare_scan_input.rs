use std::io::Write;
use std::path::Path;

use trogon_sandbox_contract::ArtifactType;
use trogon_sandbox_oci::Layer;

use super::firecracker_prepare::{prepare_firecracker, prepare_firecracker_async};
use super::{ScanInput, ScanPreparation, SecurityError};
use crate::constants::SCAN_LAYER_FILE;

/// Lays `layers` out in `dir` for a scanner, with the default extraction budgets.
pub fn prepare_scan_input(
    dir: &Path,
    artifact_type: ArtifactType,
    layers: &[Layer],
) -> Result<ScanInput, SecurityError> {
    prepare_scan_input_with_options(dir, artifact_type, layers, &ScanPreparation::default())
}

/// [`prepare_scan_input`] under the given budgets and extractor program.
pub fn prepare_scan_input_with_options(
    dir: &Path,
    artifact_type: ArtifactType,
    layers: &[Layer],
    options: &ScanPreparation,
) -> Result<ScanInput, SecurityError> {
    if artifact_type == ArtifactType::FirecrackerSnapshotV1 {
        return Ok(prepare_firecracker(dir, layers, options)?);
    }
    let path = dir.join(SCAN_LAYER_FILE);
    let mut file = std::fs::File::create(&path)?;
    for layer in layers {
        file.write_all(&layer.data)?;
    }
    Ok(ScanInput::file(path))
}

/// [`prepare_scan_input_with_options`] that waits for the extractor without blocking the runtime, and stops it
/// when the future is dropped.
pub async fn prepare_scan_input_async(
    dir: &Path,
    artifact_type: ArtifactType,
    layers: &[Layer],
    options: &ScanPreparation,
) -> Result<ScanInput, SecurityError> {
    if artifact_type == ArtifactType::FirecrackerSnapshotV1 {
        return Ok(prepare_firecracker_async(dir, layers, options).await?);
    }
    prepare_scan_input_with_options(dir, artifact_type, layers, options)
}
