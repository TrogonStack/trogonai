use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionKey(String);

impl InteractionKey {
    pub fn derive(payload: &serde_json::Value, interaction_type: &str, body: &[u8]) -> Self {
        if let Some(trigger_id) = non_empty_str(payload.get("trigger_id")) {
            return Self(trigger_id.to_owned());
        }

        if interaction_type == "view_closed"
            && let Some(view_id) = non_empty_str(payload.get("view").and_then(|view| view.get("id")))
        {
            return Self(format!("view_closed-{view_id}"));
        }

        Self(format!("interaction-{}", hex::encode(Sha256::digest(body))))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn non_empty_str(value: Option<&serde_json::Value>) -> Option<&str> {
    value.and_then(|value| value.as_str()).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests;
