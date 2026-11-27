use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use oci_client::RegistryOperation;
use oci_client::manifest::{IMAGE_MANIFEST_MEDIA_TYPE, OCI_IMAGE_MEDIA_TYPE, OciImageManifest};
use tracing::instrument;
use trogon_sandbox_contract::{Digest, OciArtifactRef};

use crate::descriptor::{descriptor_size, has_foreign_urls};
use crate::digests::verify_sha256;
use crate::limited_writer::LimitedWriter;
use crate::media_types::manifest_artifact_type;
use crate::references::digest_reference;
use crate::transport::BoundedRegistry;
use crate::{ArtifactPullLimits, Layer, ManifestSummary, OciClient, OciError, PulledArtifact, artifact_type_label};

impl OciClient {
    /// Pulls an artifact and verifies the manifest and every layer against their digests.
    ///
    /// Boxed for the same reason as [`Self::push_artifact`].
    pub fn pull_artifact<'a>(
        &'a self,
        target: &'a OciArtifactRef,
    ) -> Pin<Box<dyn Future<Output = Result<PulledArtifact, OciError>> + Send + 'a>> {
        Box::pin(self.pull_artifact_traced(target))
    }

    #[instrument(skip(self), fields(registry = %target.registry, repository = %target.repository, digest = %target.digest))]
    async fn pull_artifact_traced(&self, target: &OciArtifactRef) -> Result<PulledArtifact, OciError> {
        let started = Instant::now();
        let artifact_type = artifact_type_label(target.artifact_type);

        let result = self.pull_artifact_inner(target).await;

        if let Some(metrics) = &self.metrics {
            let pull_bytes = result.as_ref().map_or(0, |pulled| {
                pulled.layers.iter().map(|layer| layer.data.len() as u64).sum()
            });
            metrics
                .pull_bytes_total
                .with_label_values(&[target.registry.as_str(), artifact_type])
                .inc_by(pull_bytes);
            metrics
                .pull_duration_seconds
                .with_label_values(&[target.registry.as_str(), artifact_type])
                .observe(started.elapsed().as_secs_f64());
        }

        result
    }

    async fn pull_artifact_inner(&self, target: &OciArtifactRef) -> Result<PulledArtifact, OciError> {
        let reference = digest_reference(target)?;
        let auth = self.auth().auth_for(target);
        self.inner()
            .store_auth_if_needed(reference.resolve_registry(), &auth)
            .await;
        self.inner().auth(&reference, &auth, RegistryOperation::Pull).await?;

        let (manifest, _) = self.inner().pull_image_manifest(&reference, &auth).await?;

        let artifact_type = manifest_artifact_type(manifest.artifact_type.as_deref(), &manifest.config.media_type)
            .unwrap_or(target.artifact_type);

        let mut layers = Vec::with_capacity(manifest.layers.len());
        for descriptor in &manifest.layers {
            let mut data = Vec::new();
            self.inner().pull_blob(&reference, descriptor, &mut data).await?;
            verify_sha256(&data, &descriptor.digest)?;
            layers.push(Layer {
                data,
                media_type: descriptor.media_type.clone(),
                annotations: descriptor.annotations.clone().unwrap_or_default(),
            });
        }

        let summary = manifest_summary_from(&manifest, target.digest.clone(), artifact_type);
        Ok(PulledArtifact {
            reference: OciArtifactRef {
                registry: target.registry.clone(),
                repository: target.repository.clone(),
                digest: target.digest.clone(),
                artifact_type,
            },
            layers,
            annotations: summary.annotations.clone(),
            manifest: summary,
        })
    }

    /// Pulls an artifact within the given byte, count, and time budgets.
    ///
    /// The manifest, the authentication token, and each blob are read through a bounded
    /// transport that stops at the budget instead of at end of stream, never follows redirects
    /// for metadata, and sends registry credentials only to the registry origin.
    pub async fn pull_artifact_bounded(
        &self,
        target: &OciArtifactRef,
        limits: ArtifactPullLimits,
    ) -> Result<PulledArtifact, OciError> {
        tokio::time::timeout(limits.timeout, self.pull_artifact_bounded_inner(target, limits))
            .await
            .map_err(|_| OciError::RegistryRejected("bounded OCI artifact pull timed out".into()))?
    }

    async fn pull_artifact_bounded_inner(
        &self,
        target: &OciArtifactRef,
        limits: ArtifactPullLimits,
    ) -> Result<PulledArtifact, OciError> {
        let reference = digest_reference(target)?;
        let auth = self.auth().auth_for(target);
        let mut transport = self.transport(&reference, auth, tokio::time::Instant::now() + limits.timeout)?;
        let (raw, _) = transport.manifest(target.digest.as_str()).await?;
        let manifest: OciImageManifest =
            serde_json::from_slice(&raw).map_err(|error| OciError::Layout(error.to_string()))?;
        let artifact_type = manifest_artifact_type(manifest.artifact_type.as_deref(), &manifest.config.media_type)
            .ok_or_else(|| OciError::UnsupportedMediaType("artifact type is missing or unknown".into()))?;
        if artifact_type != target.artifact_type {
            return Err(OciError::UnsupportedMediaType(
                "artifact type differs from requested reference".into(),
            ));
        }
        check_layer_budget(&manifest, limits)?;
        let layers = pull_layers(&mut transport, &manifest, limits).await?;
        let annotations = manifest.annotations.clone().unwrap_or_default();
        Ok(PulledArtifact {
            reference: target.clone(),
            layers,
            annotations: annotations.clone(),
            manifest: ManifestSummary {
                digest: target.digest.clone(),
                artifact_type,
                media_type: manifest.media_type.unwrap_or_else(|| OCI_IMAGE_MEDIA_TYPE.into()),
                size: raw.len() as u64,
                annotations,
            },
        })
    }

    /// Reads the manifest of an artifact without downloading its layers.
    #[instrument(skip(self), fields(registry = %target.registry, repository = %target.repository, digest = %target.digest))]
    pub async fn head_manifest(&self, target: &OciArtifactRef) -> Result<ManifestSummary, OciError> {
        let reference = digest_reference(target)?;
        let mut transport = self.transport(
            &reference,
            self.auth().auth_for(target),
            crate::client::metadata_deadline(),
        )?;
        let (raw, digest) = transport.manifest(target.digest.as_str()).await?;
        let manifest: OciImageManifest =
            serde_json::from_slice(&raw).map_err(|_| OciError::Layout("invalid artifact manifest".into()))?;
        Ok(manifest_summary_from(&manifest, digest, target.artifact_type))
    }
}

fn check_layer_budget(manifest: &OciImageManifest, limits: ArtifactPullLimits) -> Result<(), OciError> {
    if manifest.layers.len() > limits.max_layers {
        return Err(OciError::Config("OCI artifact exceeds layer-count limit".into()));
    }
    let mut total = 0u64;
    for descriptor in &manifest.layers {
        let size = descriptor_size(descriptor)?;
        if size > limits.max_layer_bytes {
            return Err(OciError::Config("OCI artifact layer exceeds byte limit".into()));
        }
        total = total
            .checked_add(size)
            .ok_or_else(|| OciError::Config("OCI artifact sizes overflow u64".into()))?;
        if total > limits.max_total_bytes {
            return Err(OciError::Config("OCI artifact exceeds total byte limit".into()));
        }
        if has_foreign_urls(descriptor) {
            return Err(OciError::Config("foreign OCI artifact URLs are not supported".into()));
        }
        Digest::new(&descriptor.digest)?;
    }
    Ok(())
}

async fn pull_layers(
    transport: &mut BoundedRegistry,
    manifest: &OciImageManifest,
    limits: ArtifactPullLimits,
) -> Result<Vec<Layer>, OciError> {
    let mut layers = Vec::with_capacity(manifest.layers.len());
    for descriptor in &manifest.layers {
        let size = descriptor_size(descriptor)?;
        let mut writer = LimitedWriter {
            inner: Vec::new(),
            remaining: size,
        };
        transport.blob(descriptor, limits.max_layer_bytes, &mut writer).await?;
        verify_sha256(&writer.inner, &descriptor.digest)?;
        layers.push(Layer {
            data: writer.inner,
            media_type: descriptor.media_type.clone(),
            annotations: descriptor.annotations.clone().unwrap_or_default(),
        });
    }
    Ok(layers)
}

fn manifest_summary_from(
    manifest: &OciImageManifest,
    digest: Digest,
    fallback_type: trogon_sandbox_contract::ArtifactType,
) -> ManifestSummary {
    let artifact_type =
        manifest_artifact_type(manifest.artifact_type.as_deref(), &manifest.config.media_type).unwrap_or(fallback_type);
    ManifestSummary {
        digest,
        artifact_type,
        media_type: manifest
            .media_type
            .clone()
            .unwrap_or_else(|| IMAGE_MANIFEST_MEDIA_TYPE.to_string()),
        size: u64::try_from(manifest.config.size).unwrap_or_default(),
        annotations: manifest.annotations.clone().unwrap_or_default(),
    }
}
