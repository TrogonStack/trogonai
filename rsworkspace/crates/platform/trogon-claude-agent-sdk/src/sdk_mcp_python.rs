//! The Python dialect of `LocalMcpServer`: the `mcp.server.Server` that `create_sdk_mcp_server` builds, as the
//! Python SDK bridge answers it, with its `jsonschema` error selection and its tool content conversion.

use super::{LocalMcpSession, McpServerError, RequestId, ServerState, tool_error, typescript::initialize_issues};
use crate::{
    CallbackError, CancellationToken, JsonObject, McpToolRequest, ToolResultStatus, constants,
    python_json::{float_repr, python_repr},
};
use serde_json::Value;
use std::{cmp::Reverse, sync::atomic::Ordering};

/// `_build_input_schema`: a schema with a string `type` and `properties` is sent as declared; any other mapping
/// is read as parameter names whose values are not Python types, each of which becomes a required string.
pub(crate) fn input_schema(declared: &JsonObject) -> JsonObject {
    if declared.get("type").is_some_and(Value::is_string) && declared.contains_key("properties") {
        return declared.clone();
    }
    let string = || Value::Object(JsonObject::from_iter([("type".into(), "string".into())]));
    let properties = declared.keys().map(|name| (name.clone(), string())).collect();
    let required = declared.keys().map(|name| Value::from(name.as_str())).collect();
    JsonObject::from_iter([
        ("type".into(), "object".into()),
        ("properties".into(), Value::Object(properties)),
        ("required".into(), Value::Array(required)),
    ])
}

pub(crate) async fn request(
    session: &LocalMcpSession,
    message: JsonObject,
    signal: CancellationToken,
) -> Result<Option<JsonObject>, McpServerError> {
    if session.stopped.load(Ordering::SeqCst) {
        // The bridge answers every message with the failure until an `initialize` starts a new session.
        if message.get("method").and_then(Value::as_str) != Some("initialize") {
            return Err(McpServerError::Protocol(format!(
                "SDK MCP server {} stopped: its run() returned",
                python_repr(session.state.name.as_str())
            )));
        }
        session.initialized.store(false, Ordering::SeqCst);
        session.stopped.store(false, Ordering::SeqCst);
    }
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(McpServerError::Protocol("jsonrpc must be \"2.0\"".into()));
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // A response to the server is read and dropped; anything else fails to parse.
        if message.get("method").is_none() && (message.contains_key("result") || message.contains_key("error")) {
            return Ok(None);
        }
        return Err(McpServerError::Protocol("method must be a string".into()));
    };
    let params = match message.get("params") {
        None | Some(Value::Null) => None,
        Some(Value::Object(params)) => Some(params),
        Some(_) => return Err(McpServerError::Protocol("params must be an object".into())),
    };
    let Some(id) = message.get("id").and_then(request_id) else {
        notify(session, method, params);
        return Ok(None);
    };
    if signal.is_cancelled() {
        return Err(CallbackError::Cancelled.into());
    }
    let token = signal.child_token();
    let Some(_in_flight) = session.track_new(&id, token.clone()) else {
        return Err(McpServerError::Protocol(format!(
            "Request id {} is already in flight",
            py_repr(&id.value)
        )));
    };
    let answer = tokio::select! {
        biased;
        () = token.cancelled() => None,
        answer = dispatch(session, method, params, &id, token.clone()) => Some(answer),
    };
    match answer {
        // A handler's `CancelledError` ends the server's `run()`, and the bridge closes every request in flight.
        Some(Err(McpServerError::Callback(CallbackError::Cancelled))) if !signal.is_cancelled() => {
            session.stop();
            Ok(Some(failure(&id, -32000, "Connection closed")))
        }
        None if signal.is_cancelled() => Err(CallbackError::Cancelled.into()),
        None if session.stopped.load(Ordering::SeqCst) => Ok(Some(failure(&id, -32000, "Connection closed"))),
        // The bridge answers a request that `notifications/cancelled` names, which the server leaves unanswered.
        None => Ok(Some(failure(&id, -32800, "Request cancelled"))),
        Some(answer) => answer.map(Some),
    }
}

/// The bridge's own error answer, without the `data` that a server error carries.
fn failure(id: &RequestId, code: i32, message: &str) -> JsonObject {
    let error = JsonObject::from_iter([("code".into(), code.into()), ("message".into(), message.into())]);
    JsonObject::from_iter([
        ("jsonrpc".into(), "2.0".into()),
        ("id".into(), id.value.clone()),
        ("error".into(), Value::Object(error)),
    ])
}

/// A string or an integer; `mcp` reads the id strictly, so a float or a boolean makes the message a notification.
fn request_id(id: &Value) -> Option<RequestId> {
    match id {
        Value::String(text) => Some(RequestId::text(text)),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                return Some(RequestId::number(integer));
            }
            number.as_u64().map(|integer| RequestId {
                value: Value::from(integer),
                key: format!("n:{integer}"),
            })
        }
        _ => None,
    }
}

fn notify(session: &LocalMcpSession, method: &str, params: Option<&JsonObject>) {
    match method {
        "notifications/initialized" => session.initialized.store(true, Ordering::SeqCst),
        "notifications/cancelled" => {
            if let Some(id) = params.and_then(|params| params.get("requestId")).and_then(request_id) {
                session.cancel(&id.key);
            }
        }
        _ => {}
    }
}

async fn dispatch(
    session: &LocalMcpSession,
    method: &str,
    params: Option<&JsonObject>,
    id: &RequestId,
    signal: CancellationToken,
) -> Result<JsonObject, McpServerError> {
    if !constants::PYTHON_MCP_METHODS.contains(&method) {
        return Ok(error(id, -32601, "Method not found", method));
    }
    let meta_is_invalid = params
        .and_then(|params| params.get("_meta"))
        .is_some_and(|meta| !meta.is_object() && !meta.is_null());
    let initializes = method == "initialize";
    if meta_is_invalid
        || (initializes && !initialize_issues(params).is_empty())
        || (method == "tools/call" && !call_params_are_valid(params))
        || (!initializes && method != "ping" && !session.initialized.load(Ordering::SeqCst))
    {
        return Ok(error(id, -32602, "Invalid request parameters", ""));
    }
    let state = &session.state;
    let result = match method {
        "initialize" => {
            session.initialized.store(true, Ordering::SeqCst);
            initialize_result(state, params)
        }
        "tools/list" => Value::Object(JsonObject::from_iter([(
            "tools".into(),
            Value::Array(list_tools(state)),
        )])),
        "tools/call" => call_tool(state, params, signal).await?,
        _ => Value::Object(JsonObject::new()),
    };
    Ok(JsonObject::from_iter([
        ("jsonrpc".into(), "2.0".into()),
        ("id".into(), id.value.clone()),
        ("result".into(), sort_keys(result)),
    ]))
}

fn error(id: &RequestId, code: i32, message: &str, data: &str) -> JsonObject {
    let error = JsonObject::from_iter([
        ("code".into(), code.into()),
        ("message".into(), message.into()),
        ("data".into(), data.into()),
    ]);
    JsonObject::from_iter([
        ("jsonrpc".into(), "2.0".into()),
        ("id".into(), id.value.clone()),
        ("error".into(), Value::Object(error)),
    ])
}

/// `CallToolRequestParams`: a string `name` and, when given, `arguments` that are a mapping or `null`.
fn call_params_are_valid(params: Option<&JsonObject>) -> bool {
    params.is_some_and(|params| {
        params.get("name").is_some_and(Value::is_string)
            && params
                .get("arguments")
                .is_none_or(|arguments| arguments.is_object() || arguments.is_null())
    })
}

fn initialize_result(state: &ServerState, params: Option<&JsonObject>) -> Value {
    let requested = params
        .and_then(|params| params.get("protocolVersion"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let version = constants::PYTHON_MCP_PROTOCOL_VERSIONS
        .into_iter()
        .find(|supported| *supported == requested)
        .unwrap_or(constants::PYTHON_MCP_LATEST_PROTOCOL_VERSION);
    let tools = JsonObject::from_iter([("listChanged".into(), false.into())]);
    let server_info = JsonObject::from_iter([
        ("name".into(), state.name.as_str().into()),
        ("version".into(), state.version().into()),
    ]);
    Value::Object(JsonObject::from_iter([
        (
            "capabilities".into(),
            Value::Object(JsonObject::from_iter([("tools".into(), Value::Object(tools))])),
        ),
        ("protocolVersion".into(), version.into()),
        ("serverInfo".into(), Value::Object(server_info)),
    ]))
}

/// Every definition in registration order, each listed with the schema its name's last definition builds.
fn list_tools(state: &ServerState) -> Vec<Value> {
    let Some(tools) = &state.tools else {
        return Vec::new();
    };
    tools
        .values()
        .map(|tool| {
            let name = tool.name().as_str();
            // Creating the server builds the schema of every name, so each tool has one to list.
            let schema = state
                .python_schemas
                .get(name)
                .map(|schema| schema.as_json().clone())
                .unwrap_or_default();
            let mut listed = JsonObject::from_iter([
                ("name".into(), name.into()),
                ("description".into(), tool.description().into()),
                ("inputSchema".into(), Value::Object(schema)),
            ]);
            if let Some(annotations) = tool.annotations() {
                let kept = constants::PYTHON_MCP_ANNOTATION_FIELDS
                    .into_iter()
                    .filter_map(|field| {
                        annotations
                            .get(field)
                            .filter(|value| !value.is_null())
                            .map(|value| (field.to_owned(), value.clone()))
                    })
                    .collect();
                listed.insert("annotations".into(), Value::Object(kept));
            }
            Value::Object(listed)
        })
        .collect()
}

async fn call_tool(
    state: &ServerState,
    params: Option<&JsonObject>,
    signal: CancellationToken,
) -> Result<Value, McpServerError> {
    let name = params
        .and_then(|params| params.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = match params.and_then(|params| params.get("arguments")) {
        Some(Value::Object(arguments)) => arguments.clone(),
        _ => JsonObject::new(),
    };
    let tool = state.tools.as_ref().and_then(|tools| tools.get(name));
    let (Some(tool), Some(schema)) = (tool, state.python_schemas.get(name)) else {
        return Ok(tool_error(format!("Tool '{name}' not found")));
    };
    if let Some(error) = schema.compile_error() {
        return Ok(tool_error(error.to_owned()));
    }
    let instance = Value::Object(arguments.clone());
    if !schema.is_valid(&instance)
        && let Some(message) = best_match(schema.as_json(), &instance).or_else(|| schema.validation_error(&instance))
    {
        return Ok(tool_error(format!("Input validation error: {message}")));
    }
    match tool.call(McpToolRequest { arguments, signal }).await {
        Ok(output) => match convert_content(&output.content) {
            Ok(content) => Ok(Value::Object(JsonObject::from_iter([
                ("content".into(), Value::Array(content)),
                (
                    "isError".into(),
                    matches!(output.status, ToolResultStatus::Error).into(),
                ),
            ]))),
            Err(message) => Ok(tool_error(message)),
        },
        Err(CallbackError::Cancelled) => Err(CallbackError::Cancelled.into()),
        Err(CallbackError::Failed(error)) => Ok(tool_error(error.to_string())),
    }
}

/// `_convert_tool_content`; a block missing a field it needs fails like the Python `KeyError`.
fn convert_content(items: &[JsonObject]) -> Result<Vec<Value>, String> {
    let field = |item: &JsonObject, key: &str| item.get(key).cloned().ok_or_else(|| python_repr(key));
    let text = |text: Value| {
        Value::Object(JsonObject::from_iter([
            ("type".into(), "text".into()),
            ("text".into(), text),
        ]))
    };
    let mut content = Vec::new();
    for item in items {
        match item.get("type").and_then(Value::as_str) {
            Some("text") => content.push(text(field(item, "text")?)),
            Some("image") => {
                let data = field(item, "data")?;
                let mime_type = field(item, "mimeType")?;
                content.push(Value::Object(JsonObject::from_iter([
                    ("type".into(), "image".into()),
                    ("data".into(), data),
                    ("mimeType".into(), mime_type),
                ])));
            }
            Some("resource_link") => {
                let parts = ["name", "uri", "description"]
                    .into_iter()
                    .filter_map(|key| item.get(key).filter(|part| is_truthy(part)).map(py_str))
                    .collect::<Vec<_>>();
                let joined = if parts.is_empty() {
                    "Resource link".to_owned()
                } else {
                    parts.join("\n")
                };
                content.push(text(joined.into()));
            }
            Some("resource") => content.extend(resource_text(item.get("resource"))?.map(text)),
            _ => {}
        }
    }
    Ok(content)
}

/// `resource = item.get("resource") or {}` then `if "text" in resource: resource["text"]`: an embedded resource
/// that is not a mapping fails like that membership test or that index does on its Python value.
fn resource_text(resource: Option<&Value>) -> Result<Option<Value>, String> {
    match resource.filter(|resource| is_truthy(resource)) {
        Some(Value::Object(resource)) => Ok(resource.get("text").cloned()),
        Some(Value::String(resource)) if resource.contains("text") => {
            Err("string indices must be integers, not 'str'".to_owned())
        }
        Some(Value::Array(items)) if items.iter().any(|item| item.as_str() == Some("text")) => {
            Err("list indices must be integers or slices, not str".to_owned())
        }
        Some(Value::Bool(_)) => Err("argument of type 'bool' is not iterable".to_owned()),
        Some(Value::Number(number)) => Err(format!(
            "argument of type '{}' is not iterable",
            if number.is_f64() { "float" } else { "int" }
        )),
        _ => Ok(None),
    }
}

/// The result `mcp` writes back, whose mappings reach the CLI with their keys sorted.
fn sort_keys(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries = object.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sort_keys(value)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sort_keys).collect()),
        other => other,
    }
}

/// One step of a `jsonschema` error path.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Segment {
    Index(usize),
    Key(String),
}

struct Failure {
    path: Vec<Segment>,
    message: String,
    matches_type: bool,
}

/// The message of `jsonschema.exceptions.best_match` over the errors of the keywords this port reads: the shallowest
/// error, then the latest sibling, then one whose instance matches its schema's `type`, keeping the first of ties.
fn best_match(schema: &JsonObject, instance: &Value) -> Option<String> {
    let mut failures = Vec::new();
    collect(schema, instance, &mut Vec::new(), &mut failures);
    let relevance = |failure: &Failure| (Reverse(failure.path.len()), failure.path.clone(), !failure.matches_type);
    let mut best: Option<&Failure> = None;
    for failure in &failures {
        if best.is_none_or(|best| relevance(failure) > relevance(best)) {
            best = Some(failure);
        }
    }
    best.map(|failure| failure.message.clone())
}

/// The errors `Draft202012Validator.iter_errors` yields, in schema keyword order.
fn collect(schema: &JsonObject, instance: &Value, at: &mut Vec<Segment>, failures: &mut Vec<Failure>) {
    let matches_type = schema_type_matches(schema, instance);
    let fail = |failures: &mut Vec<Failure>, message: String, at: &[Segment]| {
        failures.push(Failure {
            path: at.to_vec(),
            message,
            matches_type,
        });
    };
    let mut nested = Vec::new();
    for (keyword, value) in schema {
        match (keyword.as_str(), value, instance) {
            ("type", expected, _) if !matches_type => {
                let names = match expected {
                    Value::Array(names) => names.iter().map(py_repr).collect::<Vec<_>>().join(", "),
                    other => py_repr(other),
                };
                fail(
                    failures,
                    format!("{} is not of type {names}", py_repr(instance)),
                    at.as_slice(),
                );
            }
            ("properties", Value::Object(properties), Value::Object(object)) => {
                for (name, property) in properties {
                    if let Some(item) = object.get(name) {
                        nested.push((Segment::Key(name.clone()), property, item));
                    }
                }
            }
            ("required", Value::Array(names), Value::Object(object)) => {
                for name in names.iter().filter_map(Value::as_str) {
                    if !object.contains_key(name) {
                        fail(
                            failures,
                            format!("{} is a required property", python_repr(name)),
                            at.as_slice(),
                        );
                    }
                }
            }
            ("items", Value::Object(_), Value::Array(items)) if !schema.contains_key("prefixItems") => {
                for (index, item) in items.iter().enumerate() {
                    nested.push((Segment::Index(index), value, item));
                }
            }
            ("enum", Value::Array(options), _) => {
                if !options.iter().any(|option| py_equal(option, instance)) {
                    fail(
                        failures,
                        format!("{} is not one of {}", py_repr(instance), py_repr(value)),
                        at.as_slice(),
                    );
                }
            }
            ("const", constant, _) => {
                if !py_equal(constant, instance) {
                    fail(failures, format!("{} was expected", py_repr(constant)), at.as_slice());
                }
            }
            ("minLength", Value::Number(limit), Value::String(text)) => {
                if let Some(limit) = limit.as_f64()
                    && (text.chars().count() as f64) < limit
                {
                    let message = if limit == 1.0 {
                        "should be non-empty"
                    } else {
                        "is too short"
                    };
                    fail(failures, format!("{} {message}", py_repr(instance)), at.as_slice());
                }
            }
            ("maxLength", Value::Number(limit), Value::String(text)) => {
                if let Some(limit) = limit.as_f64()
                    && (text.chars().count() as f64) > limit
                {
                    let message = if limit == 0.0 {
                        "is expected to be empty"
                    } else {
                        "is too long"
                    };
                    fail(failures, format!("{} {message}", py_repr(instance)), at.as_slice());
                }
            }
            ("minimum", Value::Number(limit), Value::Number(number)) if number.as_f64() < limit.as_f64() => {
                fail(
                    failures,
                    format!("{} is less than the minimum of {}", py_repr(instance), py_repr(value)),
                    at.as_slice(),
                );
            }
            ("maximum", Value::Number(limit), Value::Number(number)) if number.as_f64() > limit.as_f64() => {
                fail(
                    failures,
                    format!(
                        "{} is greater than the maximum of {}",
                        py_repr(instance),
                        py_repr(value)
                    ),
                    at.as_slice(),
                );
            }
            _ => {}
        }
        // Subschemas report in the position of the keyword that reaches them.
        for (segment, subschema, item) in nested.drain(..) {
            at.push(segment);
            match subschema {
                Value::Object(subschema) => collect(subschema, item, at, failures),
                Value::Bool(false) => failures.push(Failure {
                    path: at.clone(),
                    message: format!("False schema does not allow {}", py_repr(item)),
                    matches_type: false,
                }),
                _ => {}
            }
            at.pop();
        }
    }
}

/// `ValidationError._matches_type`: whether the instance has one of the types the schema names.
fn schema_type_matches(schema: &JsonObject, instance: &Value) -> bool {
    match schema.get("type") {
        Some(Value::String(name)) => is_type(instance, name),
        Some(Value::Array(names)) => names
            .iter()
            .filter_map(Value::as_str)
            .any(|name| is_type(instance, name)),
        _ => false,
    }
}

/// The draft 2020-12 type checker: an integer may be a float with no fraction, and a boolean is not a number.
fn is_type(instance: &Value, name: &str) -> bool {
    match (name, instance) {
        ("string", Value::String(_))
        | ("number", Value::Number(_))
        | ("boolean", Value::Bool(_))
        | ("null", Value::Null)
        | ("object", Value::Object(_))
        | ("array", Value::Array(_)) => true,
        ("integer", Value::Number(number)) => {
            number.is_i64() || number.is_u64() || number.as_f64().is_some_and(|float| float.fract() == 0.0)
        }
        _ => false,
    }
}

/// `jsonschema._utils.equal`: booleans never equal numbers, numbers compare by value, containers recursively.
fn py_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => match (left.as_i64(), right.as_i64()) {
            (Some(left), Some(right)) => left == right,
            _ => left.as_f64() == right.as_f64(),
        },
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(left, right)| py_equal(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .all(|(key, value)| right.get(key).is_some_and(|other| py_equal(value, other)))
        }
        _ => left == right,
    }
}

/// Python `repr` of the value `json.loads` reads from this JSON.
fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Number(number) => number
            .as_i64()
            .map(|integer| integer.to_string())
            .or_else(|| number.as_u64().map(|integer| integer.to_string()))
            .or_else(|| number.as_f64().map(float_repr))
            .unwrap_or_else(|| number.to_string()),
        Value::String(text) => python_repr(text),
        Value::Array(items) => format!("[{}]", items.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(object) => format!(
            "{{{}}}",
            object
                .iter()
                .map(|(key, value)| format!("{}: {}", python_repr(key), py_repr(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Python `str`: a string is itself, anything else reads as its `repr`.
fn py_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => py_repr(other),
    }
}

/// Python truthiness of the value `json.loads` reads from this JSON.
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|float| float != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(object) => !object.is_empty(),
    }
}
