use crate::{ModelRoute, Operation, ReviewVeto};

/// Ask only the bound local reviewer. Untrusted content never supplies authority.
#[async_trait::async_trait]
pub trait ReviewExecutor: Send {
    type Error: std::error::Error + Send + Sync + 'static;
    async fn review(
        &mut self,
        route: &ModelRoute,
        operation: Operation,
        untrusted_content: &str,
    ) -> Result<ReviewVeto, Self::Error>;
}
