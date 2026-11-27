use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_contract::OciImageRef;
use trogon_sandbox_oci::OciClient;
use trogon_sandbox_providers::{ImageUserResolver, K8sOpsError};

/// Reads an image's `USER` from its config in the registry.
pub struct OciImageUserResolver {
    oci: Arc<OciClient>,
}

impl OciImageUserResolver {
    pub fn new(oci: Arc<OciClient>) -> Self {
        Self { oci }
    }
}

#[async_trait]
impl ImageUserResolver for OciImageUserResolver {
    async fn image_user(&self, image: &str) -> Result<Option<String>, K8sOpsError> {
        let reference = OciImageRef::parse_unqualified(image);
        let resolved = self
            .oci
            .resolve_container_image(&reference)
            .await
            .map_err(|source| K8sOpsError::ImageUser {
                image: image.to_string(),
                source: Box::new(source),
            })?;
        Ok(resolved.user().map(str::to_owned))
    }
}
