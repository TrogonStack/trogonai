use oci_client::RegistryOperation;
use oci_client::manifest::{
    IMAGE_MANIFEST_MEDIA_TYPE, ImageIndexEntry, OCI_IMAGE_INDEX_MEDIA_TYPE, OCI_IMAGE_MEDIA_TYPE, OciImageIndex,
    OciImageManifest, OciManifest,
};
use serde_json::Value;
use tokio::time::Instant;
use tracing::{debug, instrument};
use trogon_sandbox_contract::{ArtifactType, Digest, OciArtifactRef};

use crate::client::metadata_deadline;
use crate::media_types::manifest_artifact_type;
use crate::references::{digest_reference, referrers_tag, subject_tag_reference};
use crate::{OciClient, OciError};

impl OciClient {
    /// Artifacts that name `subject` as their subject.
    ///
    /// Uses the referrers API and falls back to the digest-derived tag index when the API
    /// returns no entries. Entries with an unknown artifact type are skipped.
    #[instrument(skip(self), fields(registry = %subject.registry, repository = %subject.repository, digest = %subject.digest))]
    pub async fn list_referrers(&self, subject: &OciArtifactRef) -> Result<Vec<OciArtifactRef>, OciError> {
        let index = self.referrers_index(subject, metadata_deadline()).await?;

        let mut out = Vec::new();
        for entry in index.manifests {
            let Some(artifact_type) = manifest_artifact_type(entry.artifact_type.as_deref(), &entry.media_type) else {
                debug!(
                    digest = %entry.digest,
                    media_type = %entry.media_type,
                    "skipping referrer with unknown artifact type"
                );
                continue;
            };
            out.push(OciArtifactRef {
                registry: subject.registry.clone(),
                repository: subject.repository.clone(),
                digest: Digest::new(&entry.digest)?,
                artifact_type,
            });
        }
        Ok(out)
    }

    /// Adds an already pushed artifact to the referrers index of `subject`.
    #[instrument(skip(self), fields(subject = %subject.digest, referrer = %referrer.digest))]
    pub async fn attach_referrer(&self, subject: &OciArtifactRef, referrer: &OciArtifactRef) -> Result<(), OciError> {
        let referrer_ref = digest_reference(referrer)?;
        let deadline = metadata_deadline();
        let mut transport = self.transport(&referrer_ref, self.auth().auth_for(referrer), deadline)?;
        let (referrer_raw, _) = transport.manifest(referrer.digest.as_str()).await?;
        let referrer_manifest: OciImageManifest =
            serde_json::from_slice(&referrer_raw).map_err(|_| OciError::Layout("invalid referrer manifest".into()))?;

        let entry = ImageIndexEntry {
            media_type: OCI_IMAGE_MEDIA_TYPE.to_string(),
            digest: referrer.digest.to_string(),
            size: referrer_raw.len() as i64,
            platform: None,
            annotations: referrer_manifest.annotations.clone(),
            artifact_type: Some(referrer.artifact_type.media_type().to_string()),
        };
        self.publish_referrer(subject, entry, deadline).await
    }

    /// Attaches a referrer that was already pushed to the registry by digest.
    ///
    /// Use when the referrer media type is not an [`ArtifactType`], such as a signature
    /// manifest.
    #[instrument(skip(self), fields(subject = %subject.digest, referrer = %referrer_digest))]
    pub async fn attach_referrer_with_media_type(
        &self,
        subject: &OciArtifactRef,
        referrer_digest: &Digest,
        referrer_byte_size: i64,
        referrer_artifact_type: &str,
    ) -> Result<(), OciError> {
        let entry = ImageIndexEntry {
            media_type: OCI_IMAGE_MEDIA_TYPE.to_string(),
            digest: referrer_digest.to_string(),
            size: referrer_byte_size,
            platform: None,
            annotations: None,
            artifact_type: Some(referrer_artifact_type.to_string()),
        };
        self.publish_referrer(subject, entry, metadata_deadline()).await
    }

    async fn publish_referrer(
        &self,
        subject: &OciArtifactRef,
        entry: ImageIndexEntry,
        deadline: Instant,
    ) -> Result<(), OciError> {
        let mut index = self.referrers_index(subject, deadline).await?;
        let auth = self.auth().auth_for(subject);

        if !index.manifests.iter().any(|existing| existing.digest == entry.digest) {
            index.manifests.push(entry);
        }
        index.media_type = Some(OCI_IMAGE_INDEX_MEDIA_TYPE.to_string());

        let tag = referrers_tag(&subject.digest);
        let tag_ref = subject_tag_reference(subject, &tag)?;

        self.inner()
            .store_auth_if_needed(tag_ref.resolve_registry(), &auth)
            .await;
        self.inner().auth(&tag_ref, &auth, RegistryOperation::Push).await?;
        self.inner()
            .push_manifest(&tag_ref, &OciManifest::ImageIndex(index))
            .await?;

        debug!(tag = %tag, "updated referrers index");
        Ok(())
    }

    async fn referrers_index(&self, subject: &OciArtifactRef, deadline: Instant) -> Result<OciImageIndex, OciError> {
        let reference = digest_reference(subject)?;
        let mut transport = self.transport(&reference, self.auth().auth_for(subject), deadline)?;
        let index = match transport.referrers(subject.digest.as_str()).await {
            Ok(raw) => parse_referrers_index(&raw)?,
            Err(OciError::NotFound(_)) => empty_referrers_index(),
            Err(error) => return Err(error),
        };
        if !index.manifests.is_empty() {
            return Ok(index);
        }
        let (raw, digest) = match transport.manifest(&referrers_tag(&subject.digest)).await {
            Ok(manifest) => manifest,
            Err(OciError::NotFound(_)) => return Ok(index),
            Err(error) => return Err(error),
        };
        let fallback = match parse_referrers_index(&raw) {
            Ok(index) => index,
            Err(_) if digest == subject.digest && is_legacy_source_manifest(&raw, subject.artifact_type) => {
                empty_referrers_index()
            }
            Err(error) => return Err(error),
        };
        if let Some(metrics) = &self.metrics {
            metrics
                .referrers_fallback_total
                .with_label_values(&[&subject.registry])
                .inc();
        }
        Ok(fallback)
    }
}

fn empty_referrers_index() -> OciImageIndex {
    OciImageIndex {
        schema_version: 2,
        media_type: Some(OCI_IMAGE_INDEX_MEDIA_TYPE.into()),
        manifests: Vec::new(),
        annotations: None,
        artifact_type: None,
    }
}

fn parse_referrers_index(raw: &[u8]) -> Result<OciImageIndex, OciError> {
    let value: Value = serde_json::from_slice(raw).map_err(|_| OciError::Layout("invalid referrers index".into()))?;
    let manifests = value.get("manifests").and_then(Value::as_array);
    let (Some(manifests), Some(2), Some(OCI_IMAGE_INDEX_MEDIA_TYPE)) = (
        manifests,
        value.get("schemaVersion").and_then(Value::as_u64),
        value.get("mediaType").and_then(Value::as_str),
    ) else {
        return Err(OciError::Layout("referrers response is not an OCI image index".into()));
    };
    if manifests.iter().any(|entry| {
        entry
            .get("urls")
            .is_some_and(|urls| urls.as_array().is_none_or(|urls| !urls.is_empty()))
    }) {
        return Err(OciError::RegistryRejected(
            "foreign referrer URLs are not supported".into(),
        ));
    }
    serde_json::from_value(value).map_err(|_| OciError::Layout("invalid referrers index".into()))
}

/// Whether `raw` is the artifact manifest itself, which registries without a referrers index
/// leave under the digest-derived tag.
fn is_legacy_source_manifest(raw: &[u8], artifact_type: ArtifactType) -> bool {
    if !matches!(
        artifact_type,
        ArtifactType::SandboxRootfsV1
            | ArtifactType::HyperlightSnapshotV1
            | ArtifactType::FirecrackerSnapshotV1
            | ArtifactType::KubevirtDiskV1
    ) {
        return false;
    }
    let Ok(manifest) = serde_json::from_slice::<OciImageManifest>(raw) else {
        return false;
    };
    manifest.schema_version == 2
        && matches!(
            manifest.media_type.as_deref(),
            Some(OCI_IMAGE_MEDIA_TYPE | IMAGE_MANIFEST_MEDIA_TYPE)
        )
        && manifest.artifact_type.as_deref() == Some(artifact_type.media_type())
        && std::iter::once(&manifest.config)
            .chain(&manifest.layers)
            .all(|descriptor| {
                descriptor.size >= 0
                    && !descriptor.media_type.is_empty()
                    && Digest::new(&descriptor.digest).is_ok()
                    && descriptor.urls.as_ref().is_none_or(Vec::is_empty)
            })
}
