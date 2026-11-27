use async_trait::async_trait;
use trogon_sandbox_contract::OciImageRef;
use trogon_sandbox_oci::ImageReadiness;

use super::ImageReadinessChecker;

pub struct NoopImageReadinessChecker;

#[async_trait]
impl ImageReadinessChecker for NoopImageReadinessChecker {
    async fn check(&self, _image: &OciImageRef) -> anyhow::Result<ImageReadiness> {
        Ok(ImageReadiness::Ready)
    }
}
