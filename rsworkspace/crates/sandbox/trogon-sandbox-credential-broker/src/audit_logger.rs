use async_trait::async_trait;
use trogon_sandbox_contract::CredentialAuditEvent;

use crate::AuditError;

/// Where credential audit events are recorded.
#[async_trait]
pub trait AuditLogger: Send + Sync {
    async fn emit(&self, event: CredentialAuditEvent) -> Result<(), AuditError>;
}
