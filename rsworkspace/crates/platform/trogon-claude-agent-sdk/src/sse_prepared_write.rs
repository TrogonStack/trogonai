//! A line the browser transport is about to POST.

use serde_json::Value;

use crate::{JsonObject, SseLogRecord, js_text::parse_json, settings_json::js_stringify};

/// A line prepared for writing: the text to send, and the log line the preparation produced, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsePreparedWrite {
    /// The line to send. A JSON object without a non-empty string `uuid` gets one added; anything else is
    /// unchanged.
    pub line: String,
    /// A warning the preparation raised: the issued request id set overflowed and dropped an id nobody answered.
    pub log: Option<SseLogRecord>,
}

impl SsePreparedWrite {
    /// The line with a `uuid`: `line` unchanged when it is not a JSON object or already has a non-empty string
    /// `uuid`, otherwise the object with `uuid` set to `new_uuid()` (an existing `uuid` keeps its position).
    pub(crate) fn with_uuid(line: &str, object: Option<&JsonObject>, new_uuid: impl FnOnce() -> String) -> String {
        let Some(object) = object else {
            return line.to_owned();
        };
        if object
            .get("uuid")
            .and_then(Value::as_str)
            .is_some_and(|uuid| !uuid.is_empty())
        {
            return line.to_owned();
        }
        let mut stamped = object.clone();
        stamped.insert("uuid".to_owned(), Value::String(new_uuid()));
        js_stringify(&Value::Object(stamped))
    }

    /// The request body for this line: `{"session_id": ..., "events": [{"payload": <the line as JSON>}]}`.
    ///
    /// # Errors
    ///
    /// Returns the parse error when the line is not JSON.
    pub fn post_body(&self, session_id: &str) -> Result<String, serde_json::Error> {
        let payload = parse_json(&self.line, "\\ufffd")?;
        let mut event = JsonObject::new();
        event.insert("payload".to_owned(), payload);
        let mut body = JsonObject::new();
        body.insert("session_id".to_owned(), Value::String(session_id.to_owned()));
        body.insert("events".to_owned(), Value::Array(vec![Value::Object(event)]));
        Ok(js_stringify(&Value::Object(body)))
    }
}

#[cfg(test)]
#[path = "sse_prepared_write_tests.rs"]
mod tests;
