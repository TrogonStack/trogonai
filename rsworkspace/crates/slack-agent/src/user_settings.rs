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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_settings_default_has_none_fields() {
        let settings = UserSettings::default();
        assert!(settings.model.is_none());
        assert!(settings.system_prompt.is_none());
    }

    #[test]
    fn user_settings_roundtrip() {
        let settings = UserSettings {
            model: Some("claude-opus-4-6".to_string()),
            system_prompt: Some("Be helpful".to_string()),
        };
        let json = serde_json::to_string(&settings).unwrap();
        let deserialized: UserSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(deserialized.system_prompt.as_deref(), Some("Be helpful"));
    }

    #[test]
    fn user_settings_partial_roundtrip() {
        let settings = UserSettings {
            model: Some("x".to_string()),
            system_prompt: None,
        };
        let json = serde_json::to_string(&settings).unwrap();
        let deserialized: UserSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.model.as_deref(), Some("x"));
        assert!(deserialized.system_prompt.is_none());
    }

    #[test]
    fn sanitize_key_replaces_colons() {
        assert_eq!(sanitize_key("slack:dm:U123"), "slack.dm.U123");
    }

    #[test]
    fn sanitize_key_no_colons_unchanged() {
        assert_eq!(sanitize_key("U123456"), "U123456");
    }

    #[test]
    fn sanitize_key_multiple_colons() {
        assert_eq!(sanitize_key("a:b:c:d"), "a.b.c.d");
    }
}
