use super::config::TelegramWebhookSecret;
use crate::source_integration_id::SourceIntegrationId;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Clone, Default)]
pub struct TelegramRegisteredWebhookSecrets {
    by_integration: Arc<RwLock<HashMap<SourceIntegrationId, TelegramWebhookSecret>>>,
}

impl TelegramRegisteredWebhookSecrets {
    pub fn record(&self, integration: SourceIntegrationId, secret: TelegramWebhookSecret) {
        self.by_integration
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(integration, secret);
    }

    pub fn get(&self, integration: &SourceIntegrationId) -> Option<TelegramWebhookSecret> {
        self.by_integration
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(integration)
            .cloned()
    }
}
