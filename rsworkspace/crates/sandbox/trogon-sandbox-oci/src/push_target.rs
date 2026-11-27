use bytes::Bytes;
use oci_client::client::{Config, ImageLayer};
use oci_client::manifest::{IMAGE_MANIFEST_MEDIA_TYPE, OciImageManifest};
use trogon_sandbox_contract::{ArtifactType, OciArtifactRef};

use crate::constants::EMPTY_CONFIG_MEDIA_TYPE;
use crate::digests::compute_manifest_digest;
use crate::{Annotations, Layer, OciError};

pub(crate) fn empty_config() -> Config {
    Config::new(Bytes::from_static(b"{}"), EMPTY_CONFIG_MEDIA_TYPE.to_string(), None)
}

pub(crate) fn image_layers(layers: &[Layer]) -> Vec<ImageLayer> {
    layers
        .iter()
        .map(|layer| {
            ImageLayer::new(
                Bytes::from(layer.data.clone()),
                layer.media_type.clone(),
                Some(layer.annotations.clone()),
            )
        })
        .collect()
}

pub(crate) fn artifact_manifest(
    artifact_type: ArtifactType,
    oci_layers: &[ImageLayer],
    annotations: &Annotations,
) -> OciImageManifest {
    let mut manifest = OciImageManifest::build(oci_layers, &empty_config(), Some(annotations.clone()));
    manifest.artifact_type = Some(artifact_type.media_type().to_string());
    manifest.media_type = Some(IMAGE_MANIFEST_MEDIA_TYPE.to_string());
    manifest
}

/// The reference an artifact with these layers and annotations is pushed under.
///
/// The digest is the digest of the manifest that [`OciClient::push_artifact`](crate::OciClient::push_artifact)
/// builds from the same inputs.
pub fn build_push_target(
    registry: &str,
    repository: &str,
    artifact_type: ArtifactType,
    layers: &[Layer],
    annotations: &Annotations,
) -> Result<OciArtifactRef, OciError> {
    let manifest = artifact_manifest(artifact_type, &image_layers(layers), annotations);
    Ok(OciArtifactRef {
        registry: registry.to_string(),
        repository: repository.to_string(),
        digest: compute_manifest_digest(&manifest)?,
        artifact_type,
    })
}
