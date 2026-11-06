use async_trait::async_trait;

use crate::{ActivationAuditError, ActivationFact};

#[async_trait]
pub trait ActivationAudit: Send + Sync {
    async fn record(&self, fact: &ActivationFact) -> Result<(), ActivationAuditError>;
}
