use async_trait::async_trait;
use tokio::sync::Mutex;
use trogon_sandbox_contract::CredentialAuditEvent;

use crate::{AuditError, AuditLogger};

/// Audit events kept in process memory.
#[derive(Default)]
pub struct InMemoryAuditLogger {
    events: Mutex<Vec<CredentialAuditEvent>>,
}

impl InMemoryAuditLogger {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn events(&self) -> Vec<CredentialAuditEvent> {
        self.events.lock().await.clone()
    }
}

#[async_trait]
impl AuditLogger for InMemoryAuditLogger {
    async fn emit(&self, event: CredentialAuditEvent) -> Result<(), AuditError> {
        self.events.lock().await.push(event);
        Ok(())
    }
}
