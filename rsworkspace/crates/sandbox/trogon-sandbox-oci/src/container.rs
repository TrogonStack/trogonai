use std::collections::BTreeMap;
use std::path::Path;

use oci_client::config::{Architecture, ConfigFile, Os};
use oci_client::manifest::{
    IMAGE_CONFIG_MEDIA_TYPE, IMAGE_LAYER_GZIP_MEDIA_TYPE, IMAGE_LAYER_MEDIA_TYPE, OCI_IMAGE_INDEX_MEDIA_TYPE,
    OCI_IMAGE_MEDIA_TYPE, OciImageManifest, OciManifest,
};
use serde_json::Value;
use tokio::time::Instant;
use trogon_sandbox_contract::{Digest, OciImageRef};

use crate::constants::{
    CONTAINER_DEADLINE, CONTAINER_MAX_CONFIG_BYTES, DOCKER_LAYER, DOCKER_LAYER_GZIP, MAX_ENTRIES, MAX_INDEX_DEPTH,
    MAX_MANIFEST_BYTES, SOURCE_NAME_ANNOTATION,
};
use crate::descriptor::{descriptor_size, has_foreign_urls};
use crate::digests::{sha256_digest, verify_sha256};
use crate::limited_writer::LimitedWriter;
use crate::references::image_reference;
use crate::source_index::{SourceIndex, SourceManifest};
use crate::transport::BoundedRegistry;
use crate::{
    ContainerPullLimits, OciClient, OciError, RegistryHost, ResolvedContainerImage, ResolvedContainerImageError,
};

impl From<ResolvedContainerImageError> for OciError {
    fn from(error: ResolvedContainerImageError) -> Self {
        Self::Config(error.to_string())
    }
}

/// The sum of the declared blob sizes, after checking the manifest against `limits`.
fn manifest_bytes(manifest: &OciImageManifest, limits: ContainerPullLimits) -> Result<u64, OciError> {
    if manifest.config.size < 0
        || manifest.config.size > CONTAINER_MAX_CONFIG_BYTES
        || manifest.layers.len() > MAX_ENTRIES
    {
        return Err(OciError::Config(
            "OCI image exceeds config or layer-count limits".into(),
        ));
    }
    let mut total = 0u64;
    for descriptor in std::iter::once(&manifest.config).chain(&manifest.layers) {
        if has_foreign_urls(descriptor) {
            return Err(OciError::Config("foreign OCI blob URLs are not supported".into()));
        }
        let size = descriptor_size(descriptor)?;
        if size > limits.max_blob_bytes {
            return Err(OciError::Config("OCI blob exceeds size limit".into()));
        }
        total = total
            .checked_add(size)
            .ok_or_else(|| OciError::Config("OCI blob sizes overflow u64".into()))?;
        if total > limits.max_total_bytes {
            return Err(OciError::Config("OCI image exceeds download byte budget".into()));
        }
    }
    Ok(total)
}

fn has_foreign_index_urls(raw: &[u8]) -> Result<bool, OciError> {
    let value: Value = serde_json::from_slice(raw).map_err(|error| OciError::Config(error.to_string()))?;
    Ok(value["manifests"].as_array().is_some_and(|entries| {
        entries.iter().any(|entry| {
            entry
                .get("urls")
                .is_some_and(|urls| !urls.is_null() && urls.as_array().is_none_or(|urls| !urls.is_empty()))
        })
    }))
}

/// Follows image indexes down to the linux/amd64 image manifest.
async fn image_manifest(
    transport: &mut BoundedRegistry,
    selector: &str,
) -> Result<(Vec<u8>, Digest, OciImageManifest), OciError> {
    let mut selector = selector.to_owned();
    let mut expected_size = None;
    for _ in 0..MAX_INDEX_DEPTH {
        let (raw, digest) = transport.manifest(&selector).await?;
        if expected_size.is_some_and(|size| raw.len() as u64 != size) {
            return Err(OciError::Config(
                "OCI selected manifest size differs from descriptor".into(),
            ));
        }
        let parsed: OciManifest = serde_json::from_slice(&raw).map_err(|error| OciError::Config(error.to_string()))?;
        match parsed {
            OciManifest::Image(manifest) => {
                if manifest.schema_version != 2 {
                    return Err(OciError::Config("unsupported OCI manifest schema".into()));
                }
                return Ok((raw, digest, manifest));
            }
            OciManifest::ImageIndex(index) => {
                if index.schema_version != 2 || index.manifests.len() > MAX_ENTRIES {
                    return Err(OciError::Config("OCI index exceeds schema or entry limits".into()));
                }
                if has_foreign_index_urls(&raw)? {
                    return Err(OciError::Config("foreign OCI index URLs are not supported".into()));
                }
                let selected = index
                    .manifests
                    .iter()
                    .find(|entry| {
                        entry.platform.as_ref().is_some_and(|platform| {
                            platform.os == Os::Linux && platform.architecture == Architecture::Amd64
                        })
                    })
                    .or_else(|| match index.manifests.as_slice() {
                        [only] if only.platform.is_none() => Some(only),
                        _ => None,
                    })
                    .ok_or_else(|| OciError::Config("no linux/amd64 manifest in OCI index".into()))?;
                if selected.size < 0 || selected.size > MAX_MANIFEST_BYTES {
                    return Err(OciError::Config("OCI selected manifest exceeds byte limit".into()));
                }
                selector = Digest::new(&selected.digest)?.to_string();
                expected_size = Some(selected.size as u64);
            }
        }
    }
    Err(OciError::Config("OCI index nesting limit exceeded".into()))
}

impl OciClient {
    /// Resolves `image` to a digest-pinned description of its linux/amd64 variant.
    pub async fn resolve_container_image(&self, image: &OciImageRef) -> Result<ResolvedContainerImage, OciError> {
        self.resolve_container_image_with_limits(image, ContainerPullLimits::default())
            .await
    }

    pub async fn resolve_container_image_with_limits(
        &self,
        image: &OciImageRef,
        limits: ContainerPullLimits,
    ) -> Result<ResolvedContainerImage, OciError> {
        tokio::time::timeout(CONTAINER_DEADLINE, self.resolve_container_image_inner(image, limits))
            .await
            .map_err(|_| OciError::Config("OCI container pull deadline exceeded".into()))?
    }

    async fn resolve_container_image_inner(
        &self,
        image: &OciImageRef,
        limits: ContainerPullLimits,
    ) -> Result<ResolvedContainerImage, OciError> {
        RegistryHost::new(&image.registry)?;
        let reference = image_reference(image)?;
        let auth = self.auth().auth_for_registry(reference.resolve_registry())?;
        let mut transport = self.transport(&reference, auth, Instant::now() + CONTAINER_DEADLINE)?;
        let selector = image
            .digest
            .as_ref()
            .map(Digest::as_str)
            .or(image.tag.as_deref())
            .unwrap_or("latest");
        let (_, digest, manifest) = image_manifest(&mut transport, selector).await?;
        let compressed_bytes = manifest_bytes(&manifest, limits)?;
        let mut writer = LimitedWriter {
            inner: Vec::new(),
            remaining: descriptor_size(&manifest.config)?,
        };
        transport
            .blob(&manifest.config, limits.max_blob_bytes, &mut writer)
            .await?;
        let data = writer.inner;
        verify_sha256(&data, &manifest.config.digest)?;
        let config: ConfigFile = serde_json::from_slice(&data).map_err(|error| OciError::Config(error.to_string()))?;
        if config.os != Os::Linux || config.architecture != Architecture::Amd64 {
            return Err(OciError::Config(
                "Firecracker OCI conversion requires a linux/amd64 image".into(),
            ));
        }
        let config = config.config.unwrap_or_default();
        let mut env = BTreeMap::new();
        for entry in config.env.unwrap_or_default() {
            let (name, value) = entry
                .split_once('=')
                .ok_or_else(|| OciError::Config("image environment must use NAME=value".into()))?;
            env.insert(name.to_owned(), value.to_owned());
        }
        let mut pinned = image.clone();
        pinned.digest = Some(digest.clone());
        pinned.tag = None;
        Ok(ResolvedContainerImage::new(
            pinned,
            digest,
            env,
            config.working_dir.filter(|dir| !dir.is_empty()),
            config.user.filter(|user| !user.is_empty()),
            compressed_bytes,
        )?)
    }

    /// Writes the image as an OCI image layout under `destination`, verifying every blob.
    pub async fn export_container_layout(
        &self,
        image: &ResolvedContainerImage,
        destination: &Path,
    ) -> Result<(), OciError> {
        self.export_container_layout_with_limits(image, destination, ContainerPullLimits::default())
            .await
    }

    pub async fn export_container_layout_with_limits(
        &self,
        image: &ResolvedContainerImage,
        destination: &Path,
        limits: ContainerPullLimits,
    ) -> Result<(), OciError> {
        tokio::time::timeout(
            CONTAINER_DEADLINE,
            self.export_container_layout_inner(image, destination, limits),
        )
        .await
        .map_err(|_| OciError::Config("OCI container export deadline exceeded".into()))?
    }

    async fn export_container_layout_inner(
        &self,
        image: &ResolvedContainerImage,
        destination: &Path,
        limits: ContainerPullLimits,
    ) -> Result<(), OciError> {
        RegistryHost::new(&image.reference().registry)?;
        let reference = image_reference(image.reference())?;
        let auth = self.auth().auth_for_registry(reference.resolve_registry())?;
        let mut transport = self.transport(&reference, auth, Instant::now() + CONTAINER_DEADLINE)?;
        let (_, digest, mut manifest) = image_manifest(&mut transport, image.digest().as_str()).await?;
        if &digest != image.digest() {
            return Err(OciError::DigestMismatch {
                expected: image.digest().to_string(),
                actual: digest.to_string(),
            });
        }
        manifest_bytes(&manifest, limits)?;
        let blobs = destination.join("blobs/sha256");
        tokio::fs::create_dir_all(&blobs).await?;
        for descriptor in std::iter::once(&manifest.config).chain(&manifest.layers) {
            let blob_digest = Digest::new(&descriptor.digest)?;
            let size = descriptor_size(descriptor)?;
            let filename = blobs.join(blob_digest.hex());
            let file = tokio::fs::File::create(&filename).await?;
            let mut writer = LimitedWriter {
                inner: file,
                remaining: size,
            };
            transport.blob(descriptor, limits.max_blob_bytes, &mut writer).await?;
            writer.inner.sync_all().await?;
        }
        manifest.media_type = Some(OCI_IMAGE_MEDIA_TYPE.into());
        manifest.config.media_type = IMAGE_CONFIG_MEDIA_TYPE.into();
        for layer in &mut manifest.layers {
            layer.media_type = match layer.media_type.as_str() {
                DOCKER_LAYER_GZIP => IMAGE_LAYER_GZIP_MEDIA_TYPE.into(),
                DOCKER_LAYER => IMAGE_LAYER_MEDIA_TYPE.into(),
                other => other.into(),
            };
        }
        let normalized = serde_json::to_vec(&manifest).map_err(|error| OciError::Layout(error.to_string()))?;
        let normalized_digest = Digest::new(sha256_digest(&normalized))?;
        tokio::fs::write(blobs.join(normalized_digest.hex()), &normalized).await?;
        let index = SourceIndex {
            media_type: OCI_IMAGE_INDEX_MEDIA_TYPE,
            manifests: [SourceManifest {
                annotations: BTreeMap::from([(SOURCE_NAME_ANNOTATION, "source")]),
                digest: normalized_digest.as_str(),
                media_type: OCI_IMAGE_MEDIA_TYPE,
                size: normalized.len(),
            }],
            schema_version: 2,
        };
        tokio::fs::write(
            destination.join("index.json"),
            serde_json::to_vec(&index).map_err(|error| OciError::Layout(error.to_string()))?,
        )
        .await?;
        tokio::fs::write(destination.join("oci-layout"), br#"{"imageLayoutVersion":"1.0.0"}"#).await?;
        Ok(())
    }
}
