//! The `delivery_update` event of the browser transport.

use serde_json::Value;

/// A report on the delivery of an event the client wrote: the event id, a status, and when it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseDeliveryUpdate {
    /// The event the update is about.
    pub event_id: String,
    /// The delivery status.
    pub status: String,
    /// When the status was reached, when the server said so.
    pub timestamp: Option<String>,
}

impl SseDeliveryUpdate {
    /// Reads an update from parsed `data`: an object with string `event_id` and `status` and an optional string
    /// `timestamp`. Other keys are ignored and anything else is rejected.
    pub(crate) fn from_value(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let timestamp = match object.get("timestamp") {
            None => None,
            Some(Value::String(text)) => Some(text.clone()),
            Some(_) => return None,
        };
        Some(Self {
            event_id: object.get("event_id")?.as_str()?.to_owned(),
            status: object.get("status")?.as_str()?.to_owned(),
            timestamp,
        })
    }
}

#[cfg(test)]
#[path = "sse_delivery_update_tests.rs"]
mod tests;
