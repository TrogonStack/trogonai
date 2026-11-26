//! Import and export of artifacts as OCI image layouts, on disk or as a tarball.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use oci_client::manifest::{
    ImageIndexEntry, OCI_IMAGE_INDEX_MEDIA_TYPE, OCI_IMAGE_MEDIA_TYPE, OciImageIndex, OciImageManifest,
};
use serde_json::json;
use tokio::fs;
use tokio::time::Instant;
use tracing::instrument;
use trogon_sandbox_contract::{Digest, OciArtifactRef};

use crate::descriptor::descriptor_size;
use crate::digests::verify_sha256;
use crate::limited_writer::LimitedWriter;
use crate::media_types::artifact_type_from_media_type;
use crate::references::digest_reference;
use crate::{Layer, OciClient, OciError, PulledArtifact, RegistryHost, build_push_target};

const REPO_ANNOTATION: &str = "org.opencontainers.image.ref.name";
const DEFAULT_REPOSITORY: &str = "imported";
const EXPORT_DEADLINE: Duration = Duration::from_secs(120);
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// Writes `target` and its referrers into `dir` as an OCI image layout.
#[instrument(skip(client), fields(registry = %target.registry, repository = %target.repository))]
pub async fn export_to_directory(client: &OciClient, target: &OciArtifactRef, dir: &Path) -> Result<(), OciError> {
    fs::create_dir_all(dir.join("blobs/sha256")).await?;

    let pulled = client.pull_artifact(target).await?;
    let manifest_size = store_pulled_artifact(client, target, &pulled, dir).await?;

    for referrer in client.list_referrers(target).await? {
        let referrer_pulled = client.pull_artifact(&referrer).await?;
        store_pulled_artifact(client, &referrer, &referrer_pulled, dir).await?;
    }

    let mut index_annotations = BTreeMap::new();
    index_annotations.insert(REPO_ANNOTATION.to_string(), target.repository.clone());

    let index = OciImageIndex {
        schema_version: 2,
        media_type: Some(OCI_IMAGE_INDEX_MEDIA_TYPE.to_string()),
        artifact_type: None,
        annotations: Some(index_annotations),
        manifests: vec![ImageIndexEntry {
            media_type: OCI_IMAGE_MEDIA_TYPE.to_string(),
            digest: target.digest.to_string(),
            size: manifest_size as i64,
            platform: None,
            annotations: Some(pulled.manifest.annotations.clone()),
            artifact_type: Some(target.artifact_type.media_type().to_string()),
        }],
    };

    write_layout_files(dir, &index).await
}

/// Writes `target` and its referrers to `path` as a tarball of an OCI image layout.
#[instrument(skip(client), fields(registry = %target.registry, repository = %target.repository))]
pub async fn export_to_tarball(client: &OciClient, target: &OciArtifactRef, path: &Path) -> Result<(), OciError> {
    let temp = tempfile::tempdir()?;
    export_to_directory(client, target, temp.path()).await?;
    write_dir_as_tar(temp.path(), path)
}

/// Pushes every artifact of the layout in `dir` to `target_registry`.
///
/// Each pushed artifact must hash to the digest the layout lists for it.
#[instrument(skip(client), fields(target_registry = %target_registry))]
pub async fn import_from_directory(
    client: &OciClient,
    target_registry: &RegistryHost,
    dir: &Path,
) -> Result<Vec<OciArtifactRef>, OciError> {
    let index_raw = fs::read_to_string(dir.join("index.json")).await?;
    let index: OciImageIndex =
        serde_json::from_str(&index_raw).map_err(|error| OciError::Layout(format!("invalid index.json: {error}")))?;

    let repository = index
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get(REPO_ANNOTATION))
        .cloned()
        .unwrap_or_else(|| DEFAULT_REPOSITORY.to_string());

    let mut pushed = Vec::new();
    for entry in index.manifests {
        let digest = Digest::new(&entry.digest)?;
        let artifact_type = entry
            .artifact_type
            .as_deref()
            .and_then(artifact_type_from_media_type)
            .ok_or_else(|| {
                OciError::UnsupportedMediaType(format!("unknown artifact type in layout index: {}", entry.media_type))
            })?;

        let manifest_raw = fs::read(blob_path(dir, &entry.digest)).await?;
        let manifest: OciImageManifest = serde_json::from_slice(&manifest_raw)
            .map_err(|error| OciError::Layout(format!("invalid manifest blob: {error}")))?;

        let mut layers = Vec::new();
        for descriptor in &manifest.layers {
            let data = fs::read(blob_path(dir, &descriptor.digest)).await?;
            verify_sha256(&data, &descriptor.digest)?;
            layers.push(Layer {
                data,
                media_type: descriptor.media_type.clone(),
                annotations: descriptor.annotations.clone().unwrap_or_default(),
            });
        }

        let annotations = manifest.annotations.clone().unwrap_or_default();
        let target = build_push_target(
            target_registry.as_str(),
            &repository,
            artifact_type,
            &layers,
            &annotations,
        )?;

        if target.digest != digest {
            return Err(OciError::DigestMismatch {
                expected: digest.to_string(),
                actual: target.digest.to_string(),
            });
        }

        client.push_artifact(&target, layers, annotations).await?;
        pushed.push(target);
    }
    Ok(pushed)
}

/// Writes the manifest bytes exactly as the registry serves them, so the blob name is the
/// digest the layout index lists. Returns the manifest size.
async fn store_pulled_artifact(
    client: &OciClient,
    target: &OciArtifactRef,
    pulled: &PulledArtifact,
    dir: &Path,
) -> Result<usize, OciError> {
    let reference = digest_reference(target)?;
    let (auth, deadline) = (client.auth().auth_for(target), Instant::now() + EXPORT_DEADLINE);
    let mut transport = client.transport(&reference, auth, deadline)?;
    let (raw, _) = transport.manifest(target.digest.as_str()).await?;
    verify_sha256(&raw, target.digest.as_str())?;
    let manifest: OciImageManifest =
        serde_json::from_slice(&raw).map_err(|error| OciError::Layout(format!("invalid manifest: {error}")))?;

    let config_path = blob_path(dir, &manifest.config.digest);
    if !config_path.exists() {
        let mut writer = LimitedWriter {
            inner: Vec::new(),
            remaining: descriptor_size(&manifest.config)?,
        };
        transport.blob(&manifest.config, MAX_CONFIG_BYTES, &mut writer).await?;
        verify_sha256(&writer.inner, &manifest.config.digest)?;
        write_blob_at(&config_path, &writer.inner).await?;
    }

    for (layer, descriptor) in pulled.layers.iter().zip(&manifest.layers) {
        let path = blob_path(dir, &descriptor.digest);
        if !path.exists() {
            verify_sha256(&layer.data, &descriptor.digest)?;
            write_blob_at(&path, &layer.data).await?;
        }
    }

    write_blob_at(&blob_path(dir, target.digest.as_str()), &raw).await?;
    Ok(raw.len())
}

async fn write_layout_files(dir: &Path, index: &OciImageIndex) -> Result<(), OciError> {
    fs::write(
        dir.join("oci-layout"),
        json!({"imageLayoutVersion": "1.0.0"}).to_string(),
    )
    .await?;
    fs::write(
        dir.join("index.json"),
        serde_json::to_string_pretty(index).map_err(|error| OciError::Layout(error.to_string()))?,
    )
    .await?;
    Ok(())
}

async fn write_blob_at(path: &Path, data: &[u8]) -> Result<(), OciError> {
    fs::write(path, data).await?;
    Ok(())
}

fn blob_path(dir: &Path, digest: &str) -> PathBuf {
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    dir.join("blobs/sha256").join(hex)
}

fn write_dir_as_tar(src: &Path, dest: &Path) -> Result<(), OciError> {
    let mut builder = tar::Builder::new(std::fs::File::create(dest)?);
    for entry in walkdir::WalkDir::new(src).min_depth(1) {
        let entry = entry.map_err(|error| OciError::Layout(error.to_string()))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(src)
            .map_err(|error| OciError::Layout(error.to_string()))?;
        if entry.file_type().is_dir() {
            builder.append_dir(relative, path)?;
        } else {
            builder.append_path_with_name(path, relative)?;
        }
    }
    builder.finish()?;
    Ok(())
}
