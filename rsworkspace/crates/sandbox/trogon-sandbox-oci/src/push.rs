use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use oci_client::RegistryOperation;
use oci_client::manifest::{OciImageManifest, OciManifest};
use tracing::instrument;
use trogon_sandbox_contract::{Digest, OciArtifactRef};

use crate::artifact_type_label;
use crate::digests::compute_manifest_digest;
use crate::push_target::{artifact_manifest, build_push_target, empty_config, image_layers};
use crate::references::{artifact_staging_tag, digest_reference, tag_reference};
use crate::{Annotations, Layer, OciClient, OciError, PushReceipt};

impl OciClient {
    /// Pushes an artifact and pins its manifest by digest.
    ///
    /// `target.digest` must equal the digest [`build_push_target`] computes for the same
    /// layers and annotations, otherwise the push is refused before any upload.
    ///
    /// Boxed so the `Send` proof for this future is settled inside this crate. An unboxed
    /// `async fn` makes every caller re-prove it through the whole registry client, which
    /// overflows the compiler's recursion limit once the caller's own future is deep.
    pub fn push_artifact<'a>(
        &'a self,
        target: &'a OciArtifactRef,
        layers: Vec<Layer>,
        annotations: Annotations,
    ) -> Pin<Box<dyn Future<Output = Result<PushReceipt, OciError>> + Send + 'a>> {
        Box::pin(self.push_artifact_traced(target, layers, annotations))
    }

    #[instrument(skip(self, layers, annotations), fields(registry = %target.registry, repository = %target.repository))]
    async fn push_artifact_traced(
        &self,
        target: &OciArtifactRef,
        layers: Vec<Layer>,
        annotations: Annotations,
    ) -> Result<PushReceipt, OciError> {
        let started = Instant::now();
        let artifact_type = artifact_type_label(target.artifact_type);
        let push_bytes: u64 = layers.iter().map(|layer| layer.data.len() as u64).sum();

        let result = self.push_artifact_inner(target, layers, annotations).await;

        if let Some(metrics) = &self.metrics {
            metrics
                .push_bytes_total
                .with_label_values(&[target.registry.as_str(), artifact_type])
                .inc_by(push_bytes);
            metrics
                .push_duration_seconds
                .with_label_values(&[target.registry.as_str(), artifact_type])
                .observe(started.elapsed().as_secs_f64());
        }

        result
    }

    async fn push_artifact_inner(
        &self,
        target: &OciArtifactRef,
        layers: Vec<Layer>,
        annotations: Annotations,
    ) -> Result<PushReceipt, OciError> {
        let (registry, repository, kind) = (&target.registry, &target.repository, target.artifact_type);
        let computed = build_push_target(registry, repository, kind, &layers, &annotations)?;
        if computed.digest != target.digest {
            return Err(OciError::DigestMismatch {
                expected: target.digest.to_string(),
                actual: computed.digest.to_string(),
            });
        }

        let reference = tag_reference(target)?;
        let auth = self.auth().auth_for(target)?;
        self.inner()
            .store_auth_if_needed(reference.resolve_registry(), &auth)
            .await;
        self.inner().auth(&reference, &auth, RegistryOperation::Push).await?;

        let oci_layers = image_layers(&layers);
        let manifest = artifact_manifest(target.artifact_type, &oci_layers, &annotations);

        self.inner()
            .push(&reference, &oci_layers, empty_config(), &auth, Some(manifest.clone()))
            .await?;

        self.inner()
            .push_manifest(&digest_reference(target)?, &OciManifest::Image(manifest))
            .await?;

        Ok(PushReceipt {
            reference: target.clone(),
            manifest_digest: target.digest.clone(),
        })
    }

    /// Pushes a prepared image manifest with its layers and returns the manifest digest.
    pub async fn push_raw_manifest(
        &self,
        target: &OciArtifactRef,
        manifest: &OciImageManifest,
        layers: &[Layer],
    ) -> Result<Digest, OciError> {
        let digest = compute_manifest_digest(manifest)?;
        let staging_reference = crate::references::subject_tag_reference(target, &artifact_staging_tag(&digest))?;

        let auth = self.auth().auth_for(target)?;
        self.inner()
            .store_auth_if_needed(staging_reference.resolve_registry(), &auth)
            .await;
        self.inner()
            .auth(&staging_reference, &auth, RegistryOperation::Push)
            .await?;

        self.inner()
            .push(
                &staging_reference,
                &image_layers(layers),
                empty_config(),
                &auth,
                Some(manifest.clone()),
            )
            .await?;

        let digest_ref = crate::references::digest_reference_for(&target.registry, &target.repository, &digest)?;
        self.inner()
            .push_manifest(&digest_ref, &OciManifest::Image(manifest.clone()))
            .await?;

        Ok(digest)
    }
}
