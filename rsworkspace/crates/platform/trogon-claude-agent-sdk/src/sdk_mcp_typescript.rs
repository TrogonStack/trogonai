//! The TypeScript dialect of `LocalMcpServer`: the `@modelcontextprotocol/sdk` `McpServer` that
//! `createSdkMcpServer` builds, with its `Protocol` envelope handling and its zod issue reports.

use super::{LocalMcpSession, McpServerError, RequestId, ServerState, tool_error};
use crate::{
    CallbackError, CancellationToken, JsonObject, McpToolRequest, SdkTool, SdkTools,
    constants::{self, SUPPORTED_PROTOCOL_VERSIONS},
    settings_json::{js_stringify, js_stringify_pretty},
};
use serde_json::Value;

pub(crate) async fn request(
    session: &LocalMcpSession,
    message: JsonObject,
    signal: CancellationToken,
) -> Result<Option<JsonObject>, McpServerError> {
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
        .is_some_and(|meta| !meta.is_object())
    {
        return Ok(None);
    }
    let Some(id) = message.get("id") else {
        if method == "notifications/cancelled"
            && let Some(id) = params.and_then(|params| params.get("requestId")).and_then(request_id)
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
    let result = match (method, tools) {
        ("initialize", _) => {
            let issues = initialize_issues(params);
            if !issues.is_empty() {
                return Ok(error(id, -32603, pretty(&issues)));
            }
            Value::Object(initialize_result(state, params))
        }
        ("ping", _) => Value::Object(JsonObject::new()),
        ("tools/list", Some(tools)) => Value::Object(JsonObject::from_iter([(
            "tools".into(),
            Value::Array(list_tools(state, tools)),
        )])),
        ("tools/call", Some(_)) => {
            let issues = call_issues(params);
            if !issues.is_empty() {
                return Ok(error(id, -32603, pretty(&issues)));
            }
            call_tool(state, params, signal).await?
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

async fn call_tool(
    state: &ServerState,
    params: Option<&JsonObject>,
    signal: CancellationToken,
) -> Result<Value, McpServerError> {
    let name = params
        .and_then(|params| params.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = params.and_then(|params| params.get("arguments"));
    let Some(tool) = state.tools.as_ref().and_then(|tools| tools.get(name)) else {
        return Ok(tool_error(format!("MCP error -32602: Tool {name} not found")));
    };
    let schema = tool.input_schema();
    if let Some(error) = schema.compile_error() {
        return Ok(tool_error(error.to_owned()));
    }
    let mut issues = Vec::new();
    walk(schema.as_json(), arguments, &mut Vec::new(), &mut issues);
    if issues.is_empty() && !schema.is_valid(arguments.unwrap_or(&Value::Null)) {
        issues.push(custom(Vec::new()));
    }
    if !issues.is_empty() {
        return Ok(tool_error(format!(
            "MCP error -32602: Input validation error: Invalid arguments for tool {name}: {}",
            pretty(&issues)
        )));
    }
    let arguments = arguments.and_then(Value::as_object).cloned().unwrap_or_default();
    match tool.call(McpToolRequest { arguments, signal }).await {
        Ok(output) => Ok(output.to_wire_json()?),
        Err(CallbackError::Cancelled) => Err(CallbackError::Cancelled.into()),
        Err(CallbackError::Failed(error)) => Ok(tool_error(error.to_string())),
    }
}

/// The zod issues of `CallToolRequestSchema.params`.
fn call_issues(params: Option<&JsonObject>) -> Vec<JsonObject> {
    let Some(params) = params else {
        return vec![invalid_type("object", path(&["params"]), None)];
    };
    let mut issues = Vec::new();
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
            if let Some(form) = elicitation.get("form") {
                let at = path(&["params", "capabilities", "elicitation", "form"]);
                match form {
                    Value::Object(form) => {
                        if let Some(apply) = form.get("applyDefaults")
                            && !apply.is_boolean()
                        {
                            let mut at = at;
                            at.push("applyDefaults".into());
                            issues.push(invalid_type("boolean", at, Some(apply)));
                        }
                    }
                    other => {
                        issues.push(invalid_type("object", at.clone(), Some(other)));
                        issues.push(invalid_type("record", at, Some(other)));
                    }
                }
            }
            if let Some(url) = elicitation.get("url")
                && !is_js_object(url)
            {
                issues.push(custom(path(&["params", "capabilities", "elicitation", "url"])));
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
                let mut at = path(&["params", "clientInfo", "icons"]);
                at.push(index.into());
                icon_issues(icon, at, issues);
            }
        }
        other => issues.push(invalid_type("array", path(&["params", "clientInfo", "icons"]), other)),
    }
    string_field(client_info, &BASE, "version", true, issues);
    string_field(client_info, &BASE, "websiteUrl", false, issues);
    string_field(client_info, &BASE, "description", false, issues);
}

/// The zod issues of the `Icon` object schema: `src` string, optional `mimeType` string, optional `sizes` array of
/// strings and optional `theme` of `light` or `dark`; unknown keys are stripped without an issue.
fn icon_issues(icon: &Value, at: Vec<Value>, issues: &mut Vec<JsonObject>) {
    let Value::Object(icon) = icon else {
        issues.push(invalid_type("object", at, Some(icon)));
        return;
    };
    let child = |key: &str| {
        let mut child = at.clone();
        child.push(key.into());
        child
    };
    match icon.get("src") {
        Some(Value::String(_)) => {}
        other => issues.push(invalid_type("string", child("src"), other)),
    }
    match icon.get("mimeType") {
        None | Some(Value::String(_)) => {}
        other => issues.push(invalid_type("string", child("mimeType"), other)),
    }
    match icon.get("sizes") {
        None => {}
        Some(Value::Array(sizes)) => {
            for (index, size) in sizes.iter().enumerate() {
                if !size.is_string() {
                    let mut at = child("sizes");
                    at.push(index.into());
                    issues.push(invalid_type("string", at, Some(size)));
                }
            }
        }
        other => issues.push(invalid_type("array", child("sizes"), other)),
    }
    match icon.get("theme") {
        None => {}
        Some(Value::String(theme)) if theme == "light" || theme == "dark" => {}
        Some(_) => issues.push(invalid_value(vec!["light".into(), "dark".into()], child("theme"))),
    }
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

/// The zod issues of the object schema `McpServer` derives from a tool's input schema, in schema order.
fn walk(schema: &JsonObject, value: Option<&Value>, at: &mut Vec<Value>, issues: &mut Vec<JsonObject>) {
    if let Some(options) = schema.get("enum").and_then(Value::as_array) {
        if value.is_none_or(|value| !options.iter().any(|option| js_equal(option, value))) {
            issues.push(invalid_value(options.clone(), at.clone()));
        }
        return;
    }
    if let Some(constant) = schema.get("const") {
        if value.is_none_or(|value| !js_equal(constant, value)) {
            issues.push(invalid_value(vec![constant.clone()], at.clone()));
        }
        return;
    }
    let Some(expected) = schema.get("type").and_then(Value::as_str) else {
        return;
    };
    match (expected, value) {
        ("string", Some(Value::String(_)))
        | ("number", Some(Value::Number(_)))
        | ("boolean", Some(Value::Bool(_)))
        | ("null", Some(Value::Null)) => {}
        ("integer", Some(Value::Number(number))) => {
            if !is_safe_integer(number) {
                issues.push(invalid_int(at.clone()));
            }
        }
        ("integer", other) => issues.push(invalid_type("number", at.clone(), other)),
        ("object", Some(Value::Object(object))) => walk_properties(schema, object, at, issues),
        ("array", Some(Value::Array(items))) => {
            let unconstrained = JsonObject::new();
            let item_schema = schema.get("items").and_then(Value::as_object).unwrap_or(&unconstrained);
            for (index, item) in items.iter().enumerate() {
                at.push(index.into());
                walk(item_schema, Some(item), at, issues);
                at.pop();
            }
        }
        // The schema compiled, so `expected` is one of the seven JSON Schema types.
        (_, other) => issues.push(invalid_type(expected, at.clone(), other)),
    }
}

fn walk_properties(schema: &JsonObject, object: &JsonObject, at: &mut Vec<Value>, issues: &mut Vec<JsonObject>) {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };
    let required = schema.get("required").and_then(Value::as_array);
    for (key, property) in properties {
        let value = object.get(key);
        let is_required = required.is_some_and(|names| names.iter().any(|name| name.as_str() == Some(key.as_str())));
        if value.is_none() && !is_required {
            continue;
        }
        if let Value::Object(property) = property {
            at.push(key.as_str().into());
            walk(property, value, at, issues);
            at.pop();
        }
    }
}

fn is_safe_integer(number: &serde_json::Number) -> bool {
    if let Some(integer) = number.as_i64() {
        return integer.unsigned_abs() <= constants::JS_MAX_SAFE_INTEGER.unsigned_abs();
    }
    number
        .as_f64()
        .is_some_and(|float| float.fract() == 0.0 && float.abs() <= constants::JS_MAX_SAFE_INTEGER as f64)
}

/// `===` for the JSON values a zod enum or literal holds.
fn js_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
        _ => left == right,
    }
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

fn invalid_int(at: Vec<Value>) -> JsonObject {
    JsonObject::from_iter([
        ("expected".into(), "int".into()),
        ("format".into(), "safeint".into()),
        ("code".into(), "invalid_type".into()),
        ("path".into(), Value::Array(at)),
        ("message".into(), "Invalid input: expected int, received number".into()),
    ])
}

fn invalid_value(values: Vec<Value>, at: Vec<Value>) -> JsonObject {
    let message = match values.as_slice() {
        [only] => format!("Invalid input: expected {}", stringify_primitive(only)),
        _ => format!(
            "Invalid option: expected one of {}",
            values.iter().map(stringify_primitive).collect::<Vec<_>>().join("|")
        ),
    };
    JsonObject::from_iter([
        ("code".into(), "invalid_value".into()),
        ("values".into(), Value::Array(values)),
        ("path".into(), Value::Array(at)),
        ("message".into(), message.into()),
    ])
}

fn custom(at: Vec<Value>) -> JsonObject {
    JsonObject::from_iter([
        ("code".into(), "custom".into()),
        ("path".into(), Value::Array(at)),
        ("message".into(), "Invalid input".into()),
    ])
}

/// zod `stringifyPrimitive`: a string in double quotes without escaping, anything else as `String(value)`.
fn stringify_primitive(value: &Value) -> String {
    match value {
        Value::String(text) => format!("\"{text}\""),
        other => js_string(other),
    }
}

/// `String(value)` for a JSON value; a `null`, boolean or number reads as its `JSON.stringify` text.
fn js_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| if item.is_null() { String::new() } else { js_string(item) })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
        scalar => js_stringify(scalar),
    }
}
