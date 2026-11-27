use async_trait::async_trait;
use trogon_sandbox_contract::OciImageRef;
use trogon_sandbox_oci::ImageReadiness;

#[async_trait]
pub trait ImageReadinessChecker: Send + Sync {
    async fn check(&self, image: &OciImageRef) -> anyhow::Result<ImageReadiness>;
}
