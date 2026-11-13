//! The keys Python `parse_message` requires, checked in the order it reads them, with its error texts.

use serde_json::Value;

use super::truthy;
use crate::{JsonObject, ProtocolError};

pub(super) fn check(raw: &JsonObject) -> Result<(), ProtocolError> {
    let kind = raw.get("type");
    if kind.and_then(Value::as_str) == Some("system")
        && matches!(
            raw.get("subtype").and_then(Value::as_str),
            Some("hook_started" | "hook_response")
        )
    {
        return Ok(());
    }
    if !truthy(kind) {
        return Err(invalid("Message missing 'type' field".to_owned()));
    }
    let Some(kind) = kind.and_then(Value::as_str) else {
        return Ok(());
    };
    if !matches!(
        kind,
        "user" | "assistant" | "system" | "result" | "stream_event" | "rate_limit_event" | "conversation_reset"
    ) {
        return Ok(());
    }
    let keys = Keys { message: kind };
    let missing = |key: &str| keys.missing(key);
    match kind {
        "user" => {
            let content = keys
                .object(raw, "message")?
                .get("content")
                .ok_or_else(|| missing("content"))?;
            if let Value::Array(values) = content {
                for value in values {
                    let block = content_block(value)?;
                    match block.get("type").ok_or_else(|| missing("type"))?.as_str() {
                        Some("text") => require(block, &["text"], &missing)?,
                        Some("tool_use") => require(block, &["id", "name", "input"], &missing)?,
                        Some("tool_result") => require(block, &["tool_use_id"], &missing)?,
                        _ => {}
                    }
                }
            }
        }
        "assistant" => {
            let body = keys.object(raw, "message")?;
            let content = body.get("content").ok_or_else(|| missing("content"))?;
            let Value::Array(values) = content else {
                return Err(invalid(format!(
                    "Invalid assistant content (expected list, got {})",
                    python_type(content)
                )));
            };
            for value in values {
                let block = content_block(value)?;
                match block.get("type").ok_or_else(|| missing("type"))?.as_str() {
                    Some("text") => require(block, &["text"], &missing)?,
                    Some("thinking") => require(block, &["thinking", "signature"], &missing)?,
                    Some("tool_use" | "server_tool_use") => require(block, &["id", "name", "input"], &missing)?,
                    Some("tool_result") => require(block, &["tool_use_id"], &missing)?,
                    Some("advisor_tool_result") => require(block, &["tool_use_id", "content"], &missing)?,
                    _ => {}
                }
            }
            require(body, &["model"], &missing)?;
        }
        "system" => {
            let subtype = raw.get("subtype").ok_or_else(|| missing("subtype"))?;
            match subtype.as_str() {
                Some("task_started") => require(raw, &["task_id", "description", "uuid", "session_id"], &missing)?,
                Some("task_progress") => {
                    require(
                        raw,
                        &["task_id", "description", "usage", "uuid", "session_id"],
                        &missing,
                    )?;
                }
                Some("task_notification") => require(
                    raw,
                    &["task_id", "status", "output_file", "summary", "uuid", "session_id"],
                    &missing,
                )?,
                _ => {}
            }
        }
        "result" => {
            require(
                raw,
                &[
                    "subtype",
                    "duration_ms",
                    "duration_api_ms",
                    "is_error",
                    "num_turns",
                    "session_id",
                ],
                &missing,
            )?;
            let deferred = raw.get("deferred_tool_use");
            if truthy(deferred) {
                let Some(Value::Object(deferred)) = deferred else {
                    return Err(keys.not_an_object("deferred_tool_use"));
                };
                require(deferred, &["id", "name", "input"], &missing)?;
            }
        }
        "stream_event" => require(raw, &["uuid", "session_id", "event"], &missing)?,
        "rate_limit_event" => {
            require(keys.object(raw, "rate_limit_info")?, &["status"], &missing)?;
            require(raw, &["uuid", "session_id"], &missing)?;
        }
        _ => require(raw, &["new_conversation_id", "uuid", "session_id"], &missing)?,
    }
    Ok(())
}

fn invalid(message: String) -> ProtocolError {
    ProtocolError::InvalidMessage(message)
}

fn require(object: &JsonObject, keys: &[&str], missing: &dyn Fn(&str) -> ProtocolError) -> Result<(), ProtocolError> {
    match keys.iter().find(|key| !object.contains_key(**key)) {
        Some(key) => Err(missing(key)),
        None => Ok(()),
    }
}

/// The errors for one message type.
struct Keys<'a> {
    message: &'a str,
}

impl Keys<'_> {
    fn missing(&self, key: &str) -> ProtocolError {
        invalid(format!("Missing required field in {} message: '{key}'", self.message))
    }

    /// Python indexes `data[key]` again: a `KeyError` when it is absent, a `TypeError` when it is not a
    /// dictionary. Both end the stream, so both are a parse error here.
    fn not_an_object(&self, key: &str) -> ProtocolError {
        invalid(format!("Field is not an object in {} message: '{key}'", self.message))
    }

    fn object<'v>(&self, raw: &'v JsonObject, key: &str) -> Result<&'v JsonObject, ProtocolError> {
        match raw.get(key) {
            None => Err(self.missing(key)),
            Some(Value::Object(object)) => Ok(object),
            Some(_) => Err(self.not_an_object(key)),
        }
    }
}

fn content_block(value: &Value) -> Result<&JsonObject, ProtocolError> {
    value.as_object().ok_or_else(|| {
        invalid(format!(
            "Invalid content block (expected dict, got {})",
            python_type(value)
        ))
    })
}

/// The Python type name of the value `json.loads` returns for `value`.
fn python_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) if number.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}
