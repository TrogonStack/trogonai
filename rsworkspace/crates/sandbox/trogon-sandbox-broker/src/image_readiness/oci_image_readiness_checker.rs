use async_trait::async_trait;
use trogon_sandbox_contract::OciImageRef;
use trogon_sandbox_oci::{ImageReadiness, OciClient, OciError};

use super::ImageReadinessChecker;

pub struct OciImageReadinessChecker {
    client: OciClient,
}

impl OciImageReadinessChecker {
    pub fn new(client: OciClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ImageReadinessChecker for OciImageReadinessChecker {
    async fn check(&self, image: &OciImageRef) -> anyhow::Result<ImageReadiness> {
        match self.client.resolve_image_readiness(image).await {
            Ok(readiness) => Ok(readiness),
            // Not found or unreachable: the image may still be propagating to the
            // registry, or the registry is having a bad moment. Neither rules out
            // the lease ever succeeding, so surface it as a retryable state rather
            // than a hard failure.
            Err(e @ (OciError::NotFound(_) | OciError::Network(_))) => {
                Ok(ImageReadiness::Preparing { reason: e.to_string() })
            }
            Err(e) => Err(e.into()),
        }
    }
}
