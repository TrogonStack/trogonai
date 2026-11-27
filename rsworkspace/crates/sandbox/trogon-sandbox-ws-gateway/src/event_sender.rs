use serde_json::Value;
use tokio::sync::mpsc;

/// Where a backend delivers stream events, as `(stream id, event body)` pairs.
pub type EventSender = mpsc::Sender<(String, Value)>;
