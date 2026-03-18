use serde::{Deserialize, Serialize};

/// Payload published by the Bridge to NATS when it receives a prompt from an ACP client.
///
/// Subject: `{prefix}.{session_id}.agent.prompt`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptPayload {
    /// Unique request ID — used to route events back to the calling Bridge instance.
    pub req_id: String,
    /// The ACP session ID.
    pub session_id: String,
    /// Plain-text user message extracted from the PromptRequest content blocks.
    pub user_message: String,
}

/// Events published by the Runner back to the Bridge for a specific prompt request.
///
/// Subject: `{prefix}.{session_id}.agent.prompt.events.{req_id}`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptEvent {
    /// A chunk of text produced by the model.
    TextDelta { text: String },
    /// The runner finished the turn. `stop_reason` matches Anthropic values:
    /// `"end_turn"`, `"max_tokens"`, `"cancelled"`.
    Done { stop_reason: String },
    /// The runner encountered an unrecoverable error.
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_payload_roundtrip() {
        let p = PromptPayload {
            req_id: "req-1".to_string(),
            session_id: "sess-1".to_string(),
            user_message: "hello".to_string(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let p2: PromptPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(p2.req_id, "req-1");
        assert_eq!(p2.session_id, "sess-1");
        assert_eq!(p2.user_message, "hello");
    }

    #[test]
    fn prompt_event_text_delta_tag() {
        let e = PromptEvent::TextDelta { text: "hi".to_string() };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "text_delta");
        assert_eq!(v["text"], "hi");
    }

    #[test]
    fn prompt_event_done_tag() {
        let e = PromptEvent::Done { stop_reason: "end_turn".to_string() };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "done");
        assert_eq!(v["stop_reason"], "end_turn");
    }

    #[test]
    fn prompt_event_error_tag() {
        let e = PromptEvent::Error { message: "oops".to_string() };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "error");
        assert_eq!(v["message"], "oops");
    }

    #[test]
    fn prompt_event_roundtrip_done() {
        let e = PromptEvent::Done { stop_reason: "end_turn".to_string() };
        let json = serde_json::to_string(&e).unwrap();
        let e2: PromptEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(e2, PromptEvent::Done { stop_reason } if stop_reason == "end_turn"));
    }
}
