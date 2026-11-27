use serde_json::{Map, Value};

/// One event from a command's stream.
#[derive(Debug, Clone, PartialEq)]
pub struct SseFrame {
    /// The last `id:` seen up to and including this event, which is what a reconnect
    /// sends back as `Last-Event-ID`.
    pub id: Option<String>,
    /// The event's JSON object.
    pub data: Map<String, Value>,
}
