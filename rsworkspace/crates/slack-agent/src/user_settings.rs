use async_nats::jetstream::Context as JsContext;
use async_nats::jetstream::kv::{Config as KvConfig, Store};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const KV_BUCKET: &str = "slack-user-settings";

/// Per-user preferences that override the global agent defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserSettings {
    /// Override the Claude model for this user's conversations.
    pub model: Option<String>,
    /// Override the system prompt for this user's conversations.
    pub system_prompt: Option<String>,
}

pub struct UserSettingsStore {
    store: Store,
}

impl UserSettingsStore {
    pub async fn new(js: &JsContext) -> Result<Self, async_nats::Error> {
        let store = js
            .create_key_value(KvConfig {
                bucket: KV_BUCKET.to_string(),
                max_age: Duration::from_secs(365 * 24 * 3600),
                ..Default::default()
            })
            .await?;
        Ok(Self { store })
    }

    pub async fn load(&self, user_id: &str) -> UserSettings {
        match self.store.get(sanitize_key(user_id)).await {
            Ok(Some(bytes)) => serde_json::from_slice(&bytes).unwrap_or_default(),
            _ => UserSettings::default(),
        }
    }

    pub async fn save(&self, user_id: &str, settings: &UserSettings) {
        match serde_json::to_vec(settings) {
            Ok(bytes) => {
                if let Err(e) = self.store.put(sanitize_key(user_id), bytes.into()).await {
                    tracing::warn!(error = %e, user = %user_id, "Failed to save user settings");
                }
            }
            Err(e) => tracing::warn!(error = %e, "Failed to serialize user settings"),
        }
    }
}

fn sanitize_key(user_id: &str) -> String {
    user_id.replace(':', ".")
}
