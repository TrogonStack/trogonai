use crate::{ModelPurpose, ModelRoute};

/// Trusted provider adapter. Serialize the exact route, never a payload-selected route.
/// No redirects, fallback, alternate credentials or hidden auxiliary requests are allowed.
#[async_trait::async_trait]
pub trait ModelExecutor: Send {
    type Output;
    type Error: std::error::Error + Send + Sync + 'static;
    async fn model(
        &mut self,
        purpose: ModelPurpose,
        route: &ModelRoute,
        content: &str,
    ) -> Result<Self::Output, Self::Error>;
}
