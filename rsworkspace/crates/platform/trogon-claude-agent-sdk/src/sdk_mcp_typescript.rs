//! The TypeScript dialect of `LocalMcpServer`: the `@modelcontextprotocol/sdk` `McpServer` that
//! `createSdkMcpServer` builds, with its `Protocol` envelope handling and its zod issue reports.

use super::{LocalMcpSession, McpServerError, RequestId, ServerState, tool_error};
use crate::{
    CallbackError, CancellationToken, JsonObject, McpToolRequest, SdkTool, SdkTools,
    constants::{self, SUPPORTED_PROTOCOL_VERSIONS},
    sdk_mcp_zod::{self, Input},
    settings_json::js_stringify_pretty,
};
use serde_json::Value;

pub(crate) async fn request(
    session: &LocalMcpSession,
    message: JsonObject,
    signal: CancellationToken,
) -> Result<Option<JsonObject>, McpServerError> {
    // `uj` (request) and `dj` (notification) are strict objects; a message that breaks one is dropped.
    let envelope: &[&str] = if message.contains_key("id") {
        &["jsonrpc", "id", "method", "params"]
    } else {
        &["jsonrpc", "method", "params"]
    };
    if message.keys().any(|key| !envelope.contains(&key.as_str())) {
        return Ok(None);
    }
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Ok(None);
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Ok(None);
    };
    let params = match message.get("params") {
        None => None,
        Some(Value::Object(params)) => Some(params),
        Some(_) => return Ok(None),
    };
    if params
        .and_then(|params| params.get("_meta"))
        .is_some_and(|meta| !sdk_mcp_zod::meta().parse(Input::Json(meta)).issues.is_empty())
    {
        return Ok(None);
    }
    let Some(id) = message.get("id") else {
        // `_oncancel` ignores a falsy request id, and `reason` must be a string when it is present.
        if method == "notifications/cancelled"
            && params.is_some_and(|params| params.get("reason").is_none_or(Value::is_string))
            && let Some(id) = params.and_then(|params| params.get("requestId")).and_then(request_id)
            && !is_falsy(&id.value)
        {
            session.cancel(&id.key);
        }
        return Ok(None);
    };
    let Some(id) = request_id(id) else {
        return Ok(None);
    };
    if signal.is_cancelled() {
        return Err(CallbackError::Cancelled.into());
    }
    let token = signal.child_token();
    let _in_flight = session.track(&id, token.clone());
    let answer = tokio::select! {
        biased;
        () = token.cancelled() => None,
        answer = dispatch(&session.state, method, params, &id, token.clone()) => Some(answer),
    };
    // The TypeScript `Protocol` drops the answer of a request whose abort signal fired.
    if token.is_cancelled() {
        return if signal.is_cancelled() {
            Err(CallbackError::Cancelled.into())
        } else {
            Ok(None)
        };
    }
    answer.transpose()
}

/// Whether JavaScript treats a request id as false: `0` and the empty string.
fn is_falsy(id: &Value) -> bool {
    match id {
        Value::String(text) => text.is_empty(),
        other => other.as_f64() == Some(0.0),
    }
}

/// A string or a safe integer; a number with a zero fraction is the same integer in JavaScript.
fn request_id(id: &Value) -> Option<RequestId> {
    match id {
        Value::String(text) => Some(RequestId::text(text)),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                return (integer.unsigned_abs() <= constants::JS_MAX_SAFE_INTEGER.unsigned_abs())
                    .then(|| RequestId::number(integer));
            }
            let float = number.as_f64()?;
            (float.fract() == 0.0 && float.abs() <= constants::JS_MAX_SAFE_INTEGER as f64)
                .then(|| RequestId::number(float as i64))
        }
        _ => None,
    }
}

async fn dispatch(
    state: &ServerState,
    method: &str,
    params: Option<&JsonObject>,
    id: &RequestId,
    signal: CancellationToken,
) -> Result<JsonObject, McpServerError> {
    let tools = state.tools.as_ref().filter(|tools| !tools.is_empty());
    let registered = match (method, tools) {
        ("initialize" | "ping", _) => true,
        ("tools/list" | "tools/call", Some(_)) => true,
        _ => false,
    };
    if !registered {
        return Ok(error(id, -32601, "Method not found".into()));
    }
    // `Protocol` rejects a task-augmented request before the handler parses its params.
    if params
        .and_then(|params| params.get("task"))
        .and_then(Value::as_object)
        .is_some_and(|task| task.get("ttl").is_none_or(Value::is_number))
    {
        return Ok(error(
            id,
            -32603,
            format!("Server does not support task creation (required for {method})"),
        ));
    }
    let result = match (method, tools) {
        ("initialize", _) => {
            let issues = initialize_issues(params);
            if !issues.is_empty() {
                return Ok(error(id, -32603, pretty(&issues)));
            }
            Value::Object(initialize_result(state, params))
        }
        ("ping", _) => Value::Object(JsonObject::new()),
        ("tools/list", Some(tools)) => {
            if let Some(cursor) = params.and_then(|params| params.get("cursor"))
                && !cursor.is_string()
            {
                let issues = [invalid_type("string", path(&["params", "cursor"]), Some(cursor))];
                return Ok(error(id, -32603, pretty(&issues)));
            }
            Value::Object(JsonObject::from_iter([(
                "tools".into(),
                Value::Array(list_tools(state, tools)),
            )]))
        }
        ("tools/call", Some(_)) => {
            let issues = call_issues(params);
            if !issues.is_empty() {
                return Ok(error(id, -32603, pretty(&issues)));
            }
            match call_tool(state, params, signal).await? {
                Ok(result) => result,
                Err(message) => return Ok(error(id, -32602, message)),
            }
        }
        _ => return Ok(error(id, -32601, "Method not found".into())),
    };
    Ok(JsonObject::from_iter([
        ("result".into(), result),
        ("jsonrpc".into(), "2.0".into()),
        ("id".into(), id.value.clone()),
    ]))
}

fn error(id: &RequestId, code: i32, message: String) -> JsonObject {
    let error = JsonObject::from_iter([("code".into(), code.into()), ("message".into(), message.into())]);
    JsonObject::from_iter([
        ("jsonrpc".into(), "2.0".into()),
        ("id".into(), id.value.clone()),
        ("error".into(), Value::Object(error)),
    ])
}

/// `JSON.stringify(issues, null, 2)`, the message of a zod error.
fn pretty(issues: &[JsonObject]) -> String {
    js_stringify_pretty(&Value::Array(issues.iter().cloned().map(Value::Object).collect()))
}

fn initialize_result(state: &ServerState, params: Option<&JsonObject>) -> JsonObject {
    let requested = params
        .and_then(|params| params.get("protocolVersion"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let version = SUPPORTED_PROTOCOL_VERSIONS
        .into_iter()
        .find(|supported| *supported == requested)
        .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0]);
    let capabilities = match &state.tools {
        None => JsonObject::new(),
        Some(tools) if tools.is_empty() => JsonObject::from_iter([("tools".into(), Value::Object(JsonObject::new()))]),
        Some(_) => JsonObject::from_iter([(
            "tools".into(),
            Value::Object(JsonObject::from_iter([("listChanged".into(), true.into())])),
        )]),
    };
    let server_info = JsonObject::from_iter([
        ("name".into(), state.name.as_str().into()),
        ("version".into(), state.version().into()),
    ]);
    let mut result = JsonObject::from_iter([
        ("protocolVersion".into(), version.into()),
        ("capabilities".into(), Value::Object(capabilities)),
        ("serverInfo".into(), Value::Object(server_info)),
    ]);
    if let Some(instructions) = state.instructions.as_deref().filter(|text| !text.is_empty()) {
        result.insert("instructions".into(), instructions.into());
    }
    result
}

fn list_tools(state: &ServerState, tools: &SdkTools) -> Vec<Value> {
    tools
        .typescript_order()
        .into_iter()
        .map(|tool| Value::Object(listed_tool(state, tool)))
        .collect()
}

fn listed_tool(state: &ServerState, tool: &SdkTool) -> JsonObject {
    let declared = tool.input_schema().as_json();
    let mut input_schema = JsonObject::new();
    if !declared.contains_key("$schema") {
        input_schema.insert("$schema".into(), constants::MCP_TS_JSON_SCHEMA_DRAFT_07.into());
    }
    input_schema.extend(declared.iter().map(|(key, value)| (key.clone(), value.clone())));
    let mut listed = JsonObject::from_iter([
        ("name".into(), tool.name().as_str().into()),
        ("description".into(), tool.description().into()),
        ("inputSchema".into(), Value::Object(input_schema)),
    ]);
    if let Some(annotations) = tool.annotations() {
        listed.insert("annotations".into(), Value::Object(annotations.clone()));
    }
    listed.insert(
        "execution".into(),
        Value::Object(JsonObject::from_iter([("taskSupport".into(), "forbidden".into())])),
    );
    let mut meta = JsonObject::new();
    if state.always_load {
        meta.insert("anthropic/alwaysLoad".into(), true.into());
    }
    if let Some(hint) = tool.search_hint().filter(|hint| !hint.is_empty()) {
        meta.insert("anthropic/searchHint".into(), hint.into());
    }
    if tool.always_load() {
        meta.insert("anthropic/alwaysLoad".into(), true.into());
    }
    if !meta.is_empty() {
        listed.insert("_meta".into(), Value::Object(meta));
    }
    listed
}

/// `Ok(Err(message))` is the `-32602` error `Server` answers when the handler result breaks `CallToolResultSchema`.
async fn call_tool(
    state: &ServerState,
    params: Option<&JsonObject>,
    signal: CancellationToken,
) -> Result<Result<Value, String>, McpServerError> {
    let name = params
        .and_then(|params| params.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = params.and_then(|params| params.get("arguments"));
    let Some(tool) = state.tools.as_ref().and_then(|tools| tools.get(name)) else {
        return Ok(Ok(tool_error(format!("MCP error -32602: Tool {name} not found"))));
    };
    let schema = tool.input_schema();
    if let Some(error) = schema.compile_error() {
        return Ok(Ok(tool_error(error.to_owned())));
    }
    let Ok(prepared) = tool.validation().prepare(schema.as_json()) else {
        return Ok(Ok(tool_error(format!(
            "MCP error -32602: Tool {name} has a validation choice outside its input schema"
        ))));
    };
    let parsed = sdk_mcp_zod::Node::from_schema(&prepared.schema, &prepared.callbacks)
        .parse(arguments.map_or(Input::Undefined, Input::Json));
    let mut issues = parsed.issues;
    if issues.is_empty()
        && sdk_mcp_zod::has_unmodeled(schema.as_json())
        && !schema.is_valid(arguments.unwrap_or(&Value::Null))
    {
        issues.push(custom(Vec::new()));
    }
    if !issues.is_empty() {
        return Ok(Ok(tool_error(format!(
            "MCP error -32602: Input validation error: Invalid arguments for tool {name}: {}",
            pretty(&issues)
        ))));
    }
    let arguments = match parsed.value {
        Some(Value::Object(arguments)) => arguments,
        _ => JsonObject::new(),
    };
    match tool.call(McpToolRequest { arguments, signal }).await {
        Ok(output) => {
            let wire = output.to_wire_json()?;
            let checked = sdk_mcp_zod::call_tool_result().parse(Input::Json(&wire));
            if checked.issues.is_empty() {
                Ok(Ok(checked.value.unwrap_or(wire)))
            } else {
                Ok(Err(format!(
                    "MCP error -32602: Invalid tools/call result: {}",
                    pretty(&checked.issues)
                )))
            }
        }
        Err(CallbackError::Cancelled) => Err(CallbackError::Cancelled.into()),
        Err(CallbackError::Failed(error)) => Ok(Ok(tool_error(error.to_string()))),
    }
}

/// The zod issues of `CallToolRequestSchema.params`.
fn call_issues(params: Option<&JsonObject>) -> Vec<JsonObject> {
    let Some(params) = params else {
        return vec![invalid_type("object", path(&["params"]), None)];
    };
    let mut issues = Vec::new();
    // `task` comes first in the schema; a valid one never reaches this parse.
    match params.get("task") {
        None => {}
        Some(Value::Object(task)) => {
            if let Some(ttl) = task.get("ttl")
                && !ttl.is_number()
            {
                issues.push(invalid_type("number", path(&["params", "task", "ttl"]), Some(ttl)));
            }
        }
        other => issues.push(invalid_type("object", path(&["params", "task"]), other)),
    }
    string_field(params, &["params"], "name", true, &mut issues);
    if let Some(arguments) = params.get("arguments")
        && !arguments.is_object()
    {
        issues.push(invalid_type("record", path(&["params", "arguments"]), Some(arguments)));
    }
    issues
}

/// The zod issues of `InitializeRequestSchema.params`, in the order zod reports them. The Python dialect
/// reuses them to decide whether pydantic would accept the request.
pub(crate) fn initialize_issues(params: Option<&JsonObject>) -> Vec<JsonObject> {
    let Some(params) = params else {
        return vec![invalid_type("object", path(&["params"]), None)];
    };
    let mut issues = Vec::new();
    string_field(params, &["params"], "protocolVersion", true, &mut issues);
    match params.get("capabilities") {
        Some(Value::Object(capabilities)) => capability_issues(capabilities, &mut issues),
        other => issues.push(invalid_type("object", path(&["params", "capabilities"]), other)),
    }
    match params.get("clientInfo") {
        Some(Value::Object(client_info)) => client_info_issues(client_info, &mut issues),
        other => issues.push(invalid_type("object", path(&["params", "clientInfo"]), other)),
    }
    issues
}

fn capability_issues(capabilities: &JsonObject, issues: &mut Vec<JsonObject>) {
    const BASE: [&str; 2] = ["params", "capabilities"];
    record_field(capabilities, &BASE, "experimental", issues);
    match capabilities.get("sampling") {
        None => {}
        Some(Value::Object(sampling)) => {
            for key in ["context", "tools"] {
                if sampling.get(key).is_some_and(|value| !is_js_object(value)) {
                    issues.push(custom(path(&["params", "capabilities", "sampling", key])));
                }
            }
        }
        other => issues.push(invalid_type(
            "object",
            path(&["params", "capabilities", "sampling"]),
            other,
        )),
    }
    match capabilities.get("elicitation") {
        None => {}
        Some(Value::Object(elicitation)) => {
            if let Some(form) = elicitation.get("form")
                && !form.is_object()
            {
                let at = path(&["params", "capabilities", "elicitation", "form"]);
                issues.push(invalid_type("object", at.clone(), Some(form)));
                issues.push(invalid_type("record", at, Some(form)));
            }
        }
        other => {
            let at = path(&["params", "capabilities", "elicitation"]);
            issues.push(invalid_type("object", at.clone(), other));
            issues.push(invalid_type("record", at, other));
        }
    }
    match capabilities.get("roots") {
        None => {}
        Some(Value::Object(roots)) => {
            if let Some(list_changed) = roots.get("listChanged")
                && !list_changed.is_boolean()
            {
                issues.push(invalid_type(
                    "boolean",
                    path(&["params", "capabilities", "roots", "listChanged"]),
                    Some(list_changed),
                ));
            }
        }
        other => issues.push(invalid_type(
            "object",
            path(&["params", "capabilities", "roots"]),
            other,
        )),
    }
    if let Some(tasks) = capabilities.get("tasks")
        && !tasks.is_object()
    {
        issues.push(invalid_type(
            "object",
            path(&["params", "capabilities", "tasks"]),
            Some(tasks),
        ));
    }
    record_field(capabilities, &BASE, "extensions", issues);
}

fn client_info_issues(client_info: &JsonObject, issues: &mut Vec<JsonObject>) {
    const BASE: [&str; 2] = ["params", "clientInfo"];
    string_field(client_info, &BASE, "name", true, issues);
    string_field(client_info, &BASE, "title", false, issues);
    match client_info.get("icons") {
        None => {}
        Some(Value::Array(icons)) => {
            for (index, icon) in icons.iter().enumerate() {
                if !icon.is_object() {
                    let mut at = path(&["params", "clientInfo", "icons"]);
                    at.push(index.into());
                    issues.push(invalid_type("object", at, Some(icon)));
                }
            }
        }
        other => issues.push(invalid_type("array", path(&["params", "clientInfo", "icons"]), other)),
    }
    string_field(client_info, &BASE, "version", true, issues);
    string_field(client_info, &BASE, "websiteUrl", false, issues);
    string_field(client_info, &BASE, "description", false, issues);
}

/// A string field; an optional one may be absent but not `null`.
fn string_field(object: &JsonObject, base: &[&str], key: &str, required: bool, issues: &mut Vec<JsonObject>) {
    let value = object.get(key);
    if value.is_some_and(Value::is_string) || (value.is_none() && !required) {
        return;
    }
    let mut at = path(base);
    at.push(key.into());
    issues.push(invalid_type("string", at, value));
}

/// An optional record whose values are JavaScript objects.
fn record_field(object: &JsonObject, base: &[&str], key: &str, issues: &mut Vec<JsonObject>) {
    let mut at = path(base);
    at.push(key.into());
    match object.get(key) {
        None => {}
        Some(Value::Object(record)) => {
            for (entry, value) in record {
                if !is_js_object(value) {
                    let mut entry_path = at.clone();
                    entry_path.push(entry.as_str().into());
                    issues.push(custom(entry_path));
                }
            }
        }
        other => issues.push(invalid_type("record", at, other)),
    }
}

/// `typeof value === "object" && value !== null`.
fn is_js_object(value: &Value) -> bool {
    value.is_object() || value.is_array()
}

fn path(segments: &[&str]) -> Vec<Value> {
    segments.iter().map(|segment| Value::from(*segment)).collect()
}

/// The `received` word of a zod `invalid_type` message.
fn received(value: Option<&Value>) -> &'static str {
    match value {
        None => "undefined",
        Some(Value::Null) => "null",
        Some(Value::Array(_)) => "array",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Object(_)) => "object",
    }
}

fn invalid_type(expected: &str, at: Vec<Value>, value: Option<&Value>) -> JsonObject {
    JsonObject::from_iter([
        ("expected".into(), expected.into()),
        ("code".into(), "invalid_type".into()),
        ("path".into(), Value::Array(at)),
        (
            "message".into(),
            format!("Invalid input: expected {expected}, received {}", received(value)).into(),
        ),
    ])
}

fn custom(at: Vec<Value>) -> JsonObject {
    JsonObject::from_iter([
        ("code".into(), "custom".into()),
        ("path".into(), Value::Array(at)),
        ("message".into(), "Invalid input".into()),
    ])
}
