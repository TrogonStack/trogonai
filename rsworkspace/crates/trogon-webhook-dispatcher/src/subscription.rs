use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSubscription {
    pub id: String,
    pub subject_pattern: String,
    pub url: String,
    pub secret: Option<String>,
}
