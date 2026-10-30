use crate::credential::commands::domain::CredentialRef;
use crate::source_integration_id::SourceIntegrationId;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Clone, Default)]
pub struct TelegramRegisteredWebhookCredentials {
    by_integration: Arc<RwLock<HashMap<SourceIntegrationId, CredentialRef>>>,
}

impl TelegramRegisteredWebhookCredentials {
    pub fn record(&self, integration: SourceIntegrationId, credential: CredentialRef) {
        self.by_integration
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(integration, credential);
    }

    pub fn get(&self, integration: &SourceIntegrationId) -> Option<CredentialRef> {
        self.by_integration
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(integration)
            .cloned()
    }
}
