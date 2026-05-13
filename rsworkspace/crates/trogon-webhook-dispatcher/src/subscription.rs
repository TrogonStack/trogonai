use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSubscription {
    pub id: String,
    pub subject_pattern: String,
    pub url: String,
    pub secret: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(secret: Option<&str>) -> WebhookSubscription {
        WebhookSubscription {
            id: "sub-1".into(),
            subject_pattern: "transcripts.>".into(),
            url: "https://example.com/hook".into(),
            secret: secret.map(Into::into),
        }
    }

    #[test]
    fn serde_round_trip_with_secret() {
        let original = sub(Some("my-secret"));
        let json = serde_json::to_string(&original).unwrap();
        let restored: WebhookSubscription = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, original.id);
        assert_eq!(restored.secret.as_deref(), Some("my-secret"));
    }

    #[test]
    fn serde_round_trip_without_secret() {
        let original = sub(None);
        let json = serde_json::to_string(&original).unwrap();
        let restored: WebhookSubscription = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, original.id);
        assert!(restored.secret.is_none());
    }

    #[test]
    fn secret_is_null_in_json_when_none() {
        let s = sub(None);
        let json = serde_json::to_value(&s).unwrap();
        assert!(json["secret"].is_null());
    }

    #[test]
    fn clone_produces_independent_copy() {
        let original = sub(Some("s3cr3t"));
        let cloned = original.clone();
        assert_eq!(cloned.id, original.id);
        assert_eq!(cloned.secret, original.secret);
    }
}
