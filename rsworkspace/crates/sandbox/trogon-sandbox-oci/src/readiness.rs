use oci_client::RegistryOperation;
use oci_client::config::{Architecture, ConfigFile, Os};
use oci_client::manifest::OciManifest;
use tracing::instrument;
use trogon_sandbox_contract::OciImageRef;

use crate::references::image_reference;
use crate::{ImageReadiness, OciClient, OciError};

impl OciClient {
    /// Whether `image` publishes a linux/amd64 variant.
    #[instrument(skip(self), fields(registry = %image.registry, repository = %image.repository))]
    pub async fn resolve_image_readiness(&self, image: &OciImageRef) -> Result<ImageReadiness, OciError> {
        let reference = image_reference(image)?;
        let auth = self.auth().auth_for_registry(reference.resolve_registry());

        self.inner().auth(&reference, &auth, RegistryOperation::Pull).await?;

        let (manifest, _digest) = self.inner().pull_manifest(&reference, &auth).await?;

        match manifest {
            OciManifest::ImageIndex(index) => {
                let has_linux_amd64 = index.manifests.iter().any(|entry| {
                    entry.platform.as_ref().is_some_and(|platform| {
                        platform.os == Os::Linux && platform.architecture == Architecture::Amd64
                    })
                });
                if has_linux_amd64 {
                    Ok(ImageReadiness::Ready)
                } else {
                    Ok(ImageReadiness::Incompatible {
                        reason: format!(
                            "no linux/amd64 manifest found among {} platform(s) in image index",
                            index.manifests.len()
                        ),
                    })
                }
            }
            OciManifest::Image(_) => {
                let (_manifest, _digest, config_json) =
                    self.inner().pull_manifest_and_config(&reference, &auth).await?;
                let config: ConfigFile = serde_json::from_str(&config_json)
                    .map_err(|error| OciError::Layout(format!("failed to parse image config: {error}")))?;
                if config.os == Os::Linux && config.architecture == Architecture::Amd64 {
                    Ok(ImageReadiness::Ready)
                } else {
                    Ok(ImageReadiness::Incompatible {
                        reason: format!(
                            "image platform is {}/{}, expected linux/amd64",
                            config.os, config.architecture
                        ),
                    })
                }
            }
        }
    }
}
