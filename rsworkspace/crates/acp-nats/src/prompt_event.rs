use serde::{Deserialize, Serialize};

/// A rich content block transported over NATS from Bridge to Runner.
///
/// Mirrors the ACP `ContentBlock` variants we care about, in a compact wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserContentBlock {
    /// Plain text.
    Text { text: String },
    /// Base64-encoded image.
    Image { data: String, mime_type: String },
    /// HTTP/HTTPS image URL (passed natively to the Anthropic API as a URL image source).
    ImageUrl { url: String },
    /// Reference link to a resource (shown as `[@name](uri)`).
    ResourceLink { uri: String, name: String },
    /// Embedded text resource (shown as XML context block).
    Context { uri: String, text: String },
}

/// Payload published by the Bridge to NATS when it receives a prompt from an ACP client.
///
/// Subject: `{prefix}.{session_id}.agent.prompt`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptPayload {
    /// Unique request ID — used to route events back to the calling Bridge instance.
    pub req_id: String,
    /// The ACP session ID.
    pub session_id: String,
    /// Rich content blocks from the ACP prompt (text, images, resources).
    /// Always populated by current Bridge versions.
    pub content: Vec<UserContentBlock>,
    /// Plain-text fallback for backward compatibility.
    /// Used only when `content` is empty (old Bridge versions).
    #[serde(default)]
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
    /// A chunk of the model's internal reasoning (extended thinking).
    ThinkingDelta { text: String },
    /// The runner finished the turn. `stop_reason` matches Anthropic values:
    /// `"end_turn"`, `"max_tokens"`, `"max_turn_requests"`, `"cancelled"`.
    Done { stop_reason: String },
    /// The runner encountered an unrecoverable error.
    Error { message: String },
    /// A tool call was dispatched to the tool executor.
    ToolCallStarted {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// A tool call finished executing.
    ToolCallFinished {
        id: String,
        output: String,
        #[serde(default)]
        exit_code: Option<i32>,
        #[serde(default)]
        signal: Option<String>,
    },
    /// A system-level status message (forward compatibility with Anthropic API system events).
    SystemStatus { message: String },
    /// Token usage summary for the completed turn.
    UsageUpdate {
        input_tokens: u32,
        output_tokens: u32,
        #[serde(default)]
        cache_creation_tokens: u32,
        #[serde(default)]
        cache_read_tokens: u32,
        /// Context window size for the model being used (if known).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_window: Option<u64>,
    },
    /// The agent entered plan mode via the `EnterPlanMode` tool.
    /// Carries the new mode name and the active model so the Bridge can build
    /// the full `ConfigOptionUpdate` without access to the ACP agent's config.
    ModeChanged { mode: String, model: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_payload_roundtrip() {
        let p = PromptPayload {
            req_id: "req-1".to_string(),
            session_id: "sess-1".to_string(),
            content: vec![],
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
    fn prompt_event_usage_update_tag() {
        let e = PromptEvent::UsageUpdate { input_tokens: 100, output_tokens: 50, cache_creation_tokens: 0, cache_read_tokens: 0 };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "usage_update");
        assert_eq!(v["input_tokens"], 100);
        assert_eq!(v["output_tokens"], 50);
    }

    #[test]
    fn prompt_event_roundtrip_done() {
        let e = PromptEvent::Done { stop_reason: "end_turn".to_string() };
        let json = serde_json::to_string(&e).unwrap();
        let e2: PromptEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(e2, PromptEvent::Done { stop_reason } if stop_reason == "end_turn"));
    }

    #[test]
    fn prompt_event_system_status_tag() {
        let e = PromptEvent::SystemStatus { message: "rate_limit_warning".to_string() };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "system_status");
        assert_eq!(v["message"], "rate_limit_warning");
        // Roundtrip
        let json = serde_json::to_string(&e).unwrap();
        let e2: PromptEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(e2, PromptEvent::SystemStatus { message } if message == "rate_limit_warning"));
    }

    #[test]
    fn prompt_event_mode_changed_tag() {
        let e = PromptEvent::ModeChanged {
            mode: "plan".to_string(),
            model: "claude-opus-4-6".to_string(),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "mode_changed");
        assert_eq!(v["mode"], "plan");
        assert_eq!(v["model"], "claude-opus-4-6");
    }

    #[test]
    fn prompt_event_mode_changed_roundtrip() {
        let e = PromptEvent::ModeChanged {
            mode: "plan".to_string(),
            model: "claude-sonnet-4-6".to_string(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let e2: PromptEvent = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(e2, PromptEvent::ModeChanged { ref mode, ref model }
                if mode == "plan" && model == "claude-sonnet-4-6")
        );
    }

    #[test]
    fn prompt_event_mode_changed_deserialize_from_wire() {
        // Verify the exact wire format the runner publishes can be decoded by the bridge
        let wire = r#"{"type":"mode_changed","mode":"plan","model":"claude-opus-4-6"}"#;
        let e: PromptEvent = serde_json::from_str(wire).unwrap();
        assert!(matches!(e, PromptEvent::ModeChanged { ref mode, .. } if mode == "plan"));
    }
}
