use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use trogon_sandbox_oci::Layer;

use super::extract_files::{check_deadline, create_file, set_mode};
use super::extract_job::ExtractJob;
use super::{ExtractError, ScanInput, ScanPreparation};
use crate::constants::{
    COPY_CHUNK_BYTES, FIRECRACKER_LAYER_COUNT, FIRECRACKER_LAYER_ROLE, FIRECRACKER_LAYER_ROLES, METADATA_MAX_BYTES,
    SNAPSHOT_FORMAT_VERSION,
};

/// A work directory laid out for the extractor, with the deadline it must finish by.
pub(super) struct PreparedJob {
    pub(super) input: ScanInput,
    pub(super) source: PathBuf,
    pub(super) job: PathBuf,
    pub(super) program: PathBuf,
    pub(super) expires: Instant,
}

fn layers_by_role<'a>(
    layers: &'a [Layer],
    options: &ScanPreparation,
) -> Result<BTreeMap<String, &'a Layer>, ExtractError> {
    if layers.len() != FIRECRACKER_LAYER_COUNT {
        return Err(ExtractError::Rejected(
            "Firecracker snapshot requires exact layer roles",
        ));
    }
    let mut roles = BTreeMap::new();
    let mut total = 0u64;
    for layer in layers {
        let role = layer
            .annotations
            .get(FIRECRACKER_LAYER_ROLE)
            .ok_or(ExtractError::Rejected("snapshot layer has no role"))?;
        if !FIRECRACKER_LAYER_ROLES.contains(&role.as_str()) {
            return Err(ExtractError::Rejected("unknown snapshot layer role"));
        }
        if roles.insert(role.clone(), layer).is_some() {
            return Err(ExtractError::Rejected("duplicate snapshot layer role"));
        }
        total = total
            .checked_add(layer.data.len() as u64)
            .ok_or(ExtractError::Rejected("compressed byte budget overflow"))?;
        if total > options.limits.max_compressed_bytes {
            return Err(ExtractError::Rejected("compressed snapshot byte budget exceeded"));
        }
    }
    Ok(roles)
}

pub(super) fn prepare_job(
    dir: &Path,
    layers: &[Layer],
    options: &ScanPreparation,
) -> Result<PreparedJob, ExtractError> {
    options.limits.validate()?;
    let roles = layers_by_role(layers, options)?;
    let metadata = roles
        .get("snapshot.json")
        .ok_or(ExtractError::Rejected("snapshot metadata is missing"))?;
    if metadata.data.len() > METADATA_MAX_BYTES {
        return Err(ExtractError::Rejected("snapshot metadata byte budget exceeded"));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&metadata.data)?;
    if metadata["format_version"].as_u64() != Some(SNAPSHOT_FORMAT_VERSION) {
        return Err(ExtractError::Rejected("unsupported Firecracker snapshot format"));
    }
    let rootfs = roles
        .get("rootfs.ext4")
        .ok_or(ExtractError::Rejected("snapshot rootfs is missing"))?;
    if rootfs.data.is_empty() {
        return Err(ExtractError::Rejected("snapshot rootfs is empty"));
    }
    let metadata = std::fs::symlink_metadata(dir)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || std::fs::read_dir(dir)?.next().is_some() {
        return Err(ExtractError::Rejected(
            "scanner work directory must be an empty real directory",
        ));
    }
    set_mode(dir, 0o700)?;
    let dir = std::path::absolute(dir)?;
    let source = dir.join("rootfs.blob");
    let limit_path = dir.join("extraction.json");
    let prepared = ScanInput::tree(dir.join("rootfs"));
    let expires = Instant::now() + Duration::from_millis(options.limits.timeout_ms);
    let mut blob = create_file(&source)?;
    for chunk in rootfs.data.chunks(COPY_CHUNK_BYTES) {
        check_deadline(expires)?;
        blob.write_all(chunk)?;
    }
    drop(blob);
    let job = ExtractJob {
        limits: options.limits.clone(),
        gzip: rootfs.media_type.ends_with("+gzip"),
    };
    create_file(&limit_path)?.write_all(&serde_json::to_vec(&job)?)?;
    let program = match options.extractor_program.clone() {
        Some(program) => program,
        None => std::env::current_exe()?,
    };
    Ok(PreparedJob {
        input: prepared,
        source,
        job: limit_path,
        program,
        expires,
    })
}
