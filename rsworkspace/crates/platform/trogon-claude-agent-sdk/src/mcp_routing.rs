//! How each SDK routes an `mcp_message` control request to its in-process MCP servers.

use crate::{JsRuntime, JsonObject};
use serde_json::Value;

/// `sdk.mjs` `L8`: a message with a `method` and a non-null `id` waits for the server's response; anything else
/// is handed to the server and acknowledged at once.
pub(crate) fn typescript_expects_response(message: &JsonObject) -> bool {
    message.contains_key("method") && message.get("id").is_some_and(|id| !id.is_null())
}

/// `sdk.mjs` `H8`: a message with no `id` key whose method is `notifications/tools/list_changed`.
pub(crate) fn is_tools_list_changed(message: &JsonObject) -> bool {
    !message.contains_key("id")
        && message.get("method").and_then(Value::as_str) == Some("notifications/tools/list_changed")
}

/// `sdk.mjs` `initialize`: a server whose tools changed while the manifests were captured keeps only its
/// `initializeResult`, so the CLI lists its tools again over the control channel.
pub(crate) fn trim_changed_manifests(request: &mut Value, changed: &std::collections::BTreeSet<String>) {
    let Some(manifests) = request.get_mut("sdkMcpServerManifests").and_then(Value::as_object_mut) else {
        return;
    };
    for server in changed {
        let Some(manifest) = manifests.get_mut(server).and_then(Value::as_object_mut) else {
            continue;
        };
        if !manifest.contains_key("toolsListResult") {
            continue;
        }
        let initialize = manifest.remove("initializeResult");
        manifest.clear();
        if let Some(initialize) = initialize {
            manifest.insert("initializeResult".into(), initialize);
        }
    }
}

/// The acknowledgment `sdk.mjs` answers a message it does not wait on with.
pub(crate) fn typescript_acknowledgment() -> Value {
    Value::Object(JsonObject::from_iter([
        ("jsonrpc".into(), "2.0".into()),
        ("result".into(), Value::Object(JsonObject::new())),
        ("id".into(), 0.into()),
    ]))
}

/// The acknowledgment the Python `Query` answers a notification or response with.
pub(crate) fn python_acknowledgment() -> Value {
    Value::Object(JsonObject::from_iter([
        ("jsonrpc".into(), "2.0".into()),
        ("result".into(), Value::Object(JsonObject::new())),
    ]))
}

/// The JSON-RPC error the Python `Query` answers a message it could not deliver with, under the message's `id`.
pub(crate) fn python_error(message: &JsonObject, code: i64, text: String) -> Value {
    let error = JsonObject::from_iter([("code".into(), code.into()), ("message".into(), text.into())]);
    Value::Object(JsonObject::from_iter([
        ("jsonrpc".into(), "2.0".into()),
        ("id".into(), message.get("id").cloned().unwrap_or(Value::Null)),
        ("error".into(), Value::Object(error)),
    ]))
}

/// The JavaScript `String(value)` of a field `JSON.parse` read, with a missing field as `undefined`.
pub(crate) fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::Null) => "null".to_owned(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Number(number)) => match number.as_f64() {
            // JavaScript has one number type, so `1.0` reads back as `1`.
            Some(float) if !number.is_i64() && !number.is_u64() && float.fract() == 0.0 && float.abs() < 1e21 => {
                format!("{float:.0}")
            }
            _ => number.to_string(),
        },
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                other => js_string(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
    }
}

/// The `TypeError` the runtime raises for `"method" in value` when the value is not an object: V8 names the
/// value, JavaScriptCore names the bundled parameter `e` whatever the value is.
pub(crate) fn typescript_not_an_object(value: Option<&Value>, runtime: JsRuntime) -> String {
    match runtime {
        JsRuntime::Node => format!(
            "Cannot use 'in' operator to search for 'method' in {}",
            js_string(value)
        ),
        JsRuntime::Bun => "e is not an Object. (evaluating '\"method\" in e')".to_owned(),
    }
}

#[cfg(test)]
#[path = "mcp_routing_tests.rs"]
mod tests;
