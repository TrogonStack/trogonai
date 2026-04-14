use bytes::Bytes;

/// An incoming event received from the gateway on `trogon.events.>`.
///
/// The router builds one of these from each raw NATS message before passing it
/// to the LLM routing logic.
#[derive(Debug, Clone)]
pub struct RouterEvent {
    /// The full NATS subject, e.g. `trogon.events.github.pull_request`.
    pub subject: String,
    /// Raw event payload (JSON body from the gateway).
    pub payload: Bytes,
}

impl RouterEvent {
    pub fn new(subject: impl Into<String>, payload: impl Into<Bytes>) -> Self {
        Self {
            subject: subject.into(),
            payload: payload.into(),
        }
    }

    /// The event type portion of the subject (everything after `trogon.events.`).
    ///
    /// Example: `"trogon.events.github.pull_request"` → `"github.pull_request"`
    pub fn event_type(&self) -> &str {
        self.subject
            .strip_prefix("trogon.events.")
            .unwrap_or(&self.subject)
    }

    /// Payload as a UTF-8 string for inclusion in LLM prompts.
    /// Truncates to `max_bytes` to avoid blowing up context windows.
    pub fn payload_preview(&self, max_bytes: usize) -> String {
        let raw = String::from_utf8_lossy(&self.payload);
        if raw.len() <= max_bytes {
            raw.into_owned()
        } else {
            format!("{}… [truncated]", &raw[..max_bytes])
        }
    }
}

impl From<async_nats::Message> for RouterEvent {
    fn from(msg: async_nats::Message) -> Self {
        Self {
            subject: msg.subject.to_string(),
            payload: msg.payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_strips_prefix() {
        let e = RouterEvent::new("trogon.events.github.pull_request", b"{}".as_ref());
        assert_eq!(e.event_type(), "github.pull_request");
    }

    #[test]
    fn event_type_unknown_subject_unchanged() {
        let e = RouterEvent::new("unknown.subject", b"{}".as_ref());
        assert_eq!(e.event_type(), "unknown.subject");
    }

    #[test]
    fn payload_preview_truncates() {
        let e = RouterEvent::new("s", b"hello world".as_ref());
        assert_eq!(e.payload_preview(5), "hello… [truncated]");
    }

    #[test]
    fn payload_preview_no_truncation_when_short() {
        let e = RouterEvent::new("s", b"hi".as_ref());
        assert_eq!(e.payload_preview(100), "hi");
    }
}
