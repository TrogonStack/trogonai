use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::Context;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use trogon_sandbox_oci::Layer;

use crate::snapshot::ROOTFS_LAYER_MEDIA_TYPE;

mod constants;

pub use constants::{CHECKPOINT_LAYER_ANNOTATION, CHECKPOINT_NAME_ANNOTATION};

pub fn is_checkpoint_layer(annotations: &BTreeMap<String, String>) -> bool {
    annotations
        .get(CHECKPOINT_LAYER_ANNOTATION)
        .is_some_and(|v| v == "true")
}

pub fn checkpoint_name_from_annotations(annotations: &BTreeMap<String, String>) -> Option<String> {
    annotations.get(CHECKPOINT_NAME_ANNOTATION).cloned()
}

pub fn tar_checkpoint_dir(dir: &Path) -> anyhow::Result<Vec<u8>> {
    let mut encoder = tar::Builder::new(Vec::new());
    encoder
        .append_dir_all(".", dir)
        .with_context(|| format!("tar checkpoint dir {}", dir.display()))?;
    encoder.into_inner().context("finish checkpoint tar failed")
}

pub fn untar_checkpoint(gz: &[u8], dest: &Path) -> anyhow::Result<()> {
    let mut decoder = GzDecoder::new(gz);
    let mut raw = Vec::new();
    decoder
        .read_to_end(&mut raw)
        .context("gunzip checkpoint artifact failed")?;
    std::fs::create_dir_all(dest).with_context(|| format!("create {}", dest.display()))?;
    let mut archive = tar::Archive::new(raw.as_slice());
    archive
        .unpack(dest)
        .with_context(|| format!("untar checkpoint into {}", dest.display()))?;
    Ok(())
}

pub fn gzip_checkpoint(raw_tar: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(raw_tar).context("gzip checkpoint tar failed")?;
    encoder.finish().context("gzip finish checkpoint failed")
}

pub fn checkpoint_layer(gz_tar: Vec<u8>, checkpoint_name: &str) -> Layer {
    let mut annotations = BTreeMap::new();
    annotations.insert(CHECKPOINT_LAYER_ANNOTATION.to_string(), "true".to_string());
    annotations.insert(CHECKPOINT_NAME_ANNOTATION.to_string(), checkpoint_name.to_string());
    Layer {
        data: gz_tar,
        media_type: ROOTFS_LAYER_MEDIA_TYPE.to_string(),
        annotations,
    }
}

pub fn checkpoint_storage_dir(container_id: &str, checkpoint_name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "trogon-docker-checkpoint-{container_id}-{checkpoint_name}-{}",
        crate::fresh_id::fresh_id()
    ))
}
