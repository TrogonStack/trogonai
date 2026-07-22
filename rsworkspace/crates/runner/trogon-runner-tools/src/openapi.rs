//! OpenAPI → tools connections for runners.
//!
//! Adapted from Vercel Eve's "Connections" idea: alongside MCP servers, a
//! session may declare OpenAPI 3.0 specs. Each spec operation becomes a callable
//! agent tool (`{connection}__{operationId}`). This module parses the spec,
//! synthesizes one JSON-Schema per operation (path + query + header params and
//! the JSON request body), and returns `trogon_tools::ToolDef`s plus dispatch
//! entries.
//!
//! Integration is deliberately parallel to [`crate::mcp`]: the per-connection
//! executor implements [`trogon_mcp::McpCallTool`], so OpenAPI entries push into
//! the same [`crate::mcp::McpDispatch`] table and runners route prefixed calls
//! through their existing MCP path with no extra wiring.
//!
//! MVP scope: OpenAPI 3.0 JSON, `GET`/`POST`/`PUT`/`PATCH`/`DELETE`, apiKey /
//! bearer / basic auth, mandatory include-filter for large specs. Every outbound
//! URL (spec fetch and each call) is gated by the egress [`EgressPolicy`].

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tracing::{info, warn};
use trogon_tools::ToolDef;

use crate::egress::EgressPolicy;

/// Cap on operations exposed per connection. Beyond this the tool list bloats
/// the model's context; callers should narrow with `include`. Excess is dropped
/// with a warning (never silently).
const MAX_OPS_PER_CONNECTION: usize = 64;

/// Max depth when inlining `#/components/...` `$ref`s into a synthesized schema.
const MAX_REF_DEPTH: usize = 8;

/// Where the OpenAPI document comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecSource {
    /// Fetch the spec over HTTP(S) at session start (egress-gated).
    Url(String),
    /// The spec document is embedded inline as JSON.
    Inline(Value),
}

/// How to authenticate calls to the API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum OpenApiAuth {
    #[default]
    None,
    /// `Authorization: Bearer <token>`.
    Bearer { token: String },
    /// HTTP Basic auth.
    Basic { username: String, password: String },
    /// API key sent as a header or query parameter.
    ApiKey {
        name: String,
        #[serde(default)]
        location: ApiKeyLocation,
        value: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyLocation {
    #[default]
    Header,
    Query,
}

/// A persisted OpenAPI connection (transport-neutral, mirrors `StoredMcpServer`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredOpenApiServer {
    /// Connection name — used as the tool prefix (`{name}__{operationId}`).
    pub name: String,
    /// Where to load the OpenAPI document from.
    pub spec: SpecSource,
    /// Override for the API base URL. When empty, the spec's first `servers[].url`
    /// is used (falling back to the spec URL's origin).
    #[serde(default)]
    pub base_url: String,
    /// Auth applied to every call.
    #[serde(default)]
    pub auth: OpenApiAuth,
    /// If non-empty, only operations whose `operationId` is listed are exposed.
    /// Required in practice for large specs (see [`MAX_OPS_PER_CONNECTION`]).
    #[serde(default)]
    pub include: Vec<String>,
    /// Per-request timeout in seconds.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// One resolved operation the executor can invoke.
#[derive(Debug, Clone)]
struct Operation {
    /// HTTP method, uppercased (`GET`, `POST`, …).
    method: String,
    /// Path template, e.g. `/pets/{petId}`.
    path: String,
    /// Names of `in: path` parameters (substituted into `path`).
    path_params: Vec<String>,
    /// Names of `in: query` parameters.
    query_params: Vec<String>,
    /// Names of `in: header` parameters.
    header_params: Vec<String>,
    /// Whether a JSON request body is accepted (passed by the model as `body`).
    has_body: bool,
}

/// Executes one OpenAPI connection's operations as MCP-style tool calls.
pub struct OpenApiExecutor {
    http: reqwest::Client,
    base_url: String,
    auth: OpenApiAuth,
    timeout: Option<std::time::Duration>,
    policy: EgressPolicy,
    /// operationId → operation metadata.
    ops: BTreeMap<String, Operation>,
}

impl OpenApiExecutor {
    async fn execute(&self, name: &str, args: &Value) -> Result<String, String> {
        let op = self.ops.get(name).ok_or_else(|| format!("unknown operation: {name}"))?;
        let empty = Map::new();
        let args = args.as_object().unwrap_or(&empty);

        // Substitute path parameters into the template.
        let mut path = op.path.clone();
        for p in &op.path_params {
            let v = args.get(p).map(value_to_string).unwrap_or_default();
            path = path.replace(&format!("{{{p}}}"), &percent_encode_path(&v));
        }

        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        if !self.policy.is_allowed(&url) {
            return Err(format!("URL denied by egress policy: {url}"));
        }

        let method = reqwest::Method::from_bytes(op.method.as_bytes())
            .map_err(|_| format!("invalid HTTP method: {}", op.method))?;
        let mut req = self.http.request(method, &url);
        if let Some(t) = self.timeout {
            req = req.timeout(t);
        }

        // Query parameters.
        let query: Vec<(String, String)> = op
            .query_params
            .iter()
            .filter_map(|q| args.get(q).map(|v| (q.clone(), value_to_string(v))))
            .collect();
        if !query.is_empty() {
            req = req.query(&query);
        }

        // Header parameters.
        for h in &op.header_params {
            if let Some(v) = args.get(h) {
                req = req.header(h.as_str(), value_to_string(v));
            }
        }

        // Auth.
        req = apply_auth(req, &self.auth);

        // JSON request body.
        if op.has_body && let Some(body) = args.get("body") {
            req = req
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(serde_json::to_vec(body).map_err(|e| e.to_string())?);
        }

        let resp = req.send().await.map_err(|e| format!("request failed: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        // Surface non-2xx as content (not a hard error) so the model can react to
        // the API's own error payload.
        Ok(format!("HTTP {}\n{}", status.as_u16(), text))
    }
}

impl trogon_mcp::McpCallTool for OpenApiExecutor {
    fn call_tool<'a>(
        &'a self,
        name: &'a str,
        arguments: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(self.execute(name, arguments))
    }
}

fn apply_auth(req: reqwest::RequestBuilder, auth: &OpenApiAuth) -> reqwest::RequestBuilder {
    match auth {
        OpenApiAuth::None => req,
        OpenApiAuth::Bearer { token } => req.bearer_auth(token),
        OpenApiAuth::Basic { username, password } => req.basic_auth(username, Some(password)),
        OpenApiAuth::ApiKey { name, location, value } => match location {
            ApiKeyLocation::Header => req.header(name.as_str(), value.as_str()),
            ApiKeyLocation::Query => req.query(&[(name.as_str(), value.as_str())]),
        },
    }
}

/// Render a JSON arg as a string for use in a URL/header (strings unquoted).
fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Minimal percent-encoding for a path segment value (spaces, `/`, `?`, `#`, `%`).
fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Connect each OpenAPI connection: load the spec, synthesize tools, and return
/// tool defs + dispatch entries `(prefixed_name, operationId, executor)`.
///
/// A connection that fails to load/parse, or is denied by egress, is logged and
/// skipped — a bad spec never blocks a prompt.
pub async fn build_session_openapi(
    http: &reqwest::Client,
    servers: &[StoredOpenApiServer],
    policy: &EgressPolicy,
) -> (Vec<ToolDef>, crate::mcp::McpDispatch) {
    let mut tool_defs = Vec::new();
    let mut dispatch = crate::mcp::McpDispatch::new();

    for server in servers {
        let spec = match load_spec(http, &server.spec, policy).await {
            Ok(s) => s,
            Err(e) => {
                warn!(name = %server.name, error = %e, "OpenAPI spec load failed — skipping");
                continue;
            }
        };

        let base_url = resolve_base_url(server, &spec);
        if base_url.is_empty() {
            warn!(name = %server.name, "OpenAPI base URL could not be determined — skipping");
            continue;
        }

        let (defs, ops) = parse_operations(&server.name, &spec, &server.include);
        if ops.is_empty() {
            warn!(name = %server.name, "OpenAPI spec yielded no operations — skipping");
            continue;
        }

        let timeout = server.timeout_secs.map(std::time::Duration::from_secs);
        let executor: Arc<dyn trogon_mcp::McpCallTool> = Arc::new(OpenApiExecutor {
            http: http.clone(),
            base_url,
            auth: server.auth.clone(),
            timeout,
            policy: policy.clone(),
            ops: ops.clone(),
        });

        let count = defs.len();
        for def in defs {
            // The prefixed tool name is `{name}__{operationId}`; the executor is
            // keyed by the bare operationId.
            let original = def.name.split_once("__").map(|(_, o)| o.to_string()).unwrap_or_else(|| def.name.clone());
            dispatch.push((def.name.clone(), original, executor.clone()));
            tool_defs.push(def);
        }
        info!(name = %server.name, tools = count, "OpenAPI connection loaded");
    }

    (tool_defs, dispatch)
}

async fn load_spec(
    http: &reqwest::Client,
    source: &SpecSource,
    policy: &EgressPolicy,
) -> Result<Value, String> {
    match source {
        SpecSource::Inline(v) => Ok(v.clone()),
        SpecSource::Url(url) => {
            if !policy.is_allowed(url) {
                return Err(format!("spec URL denied by egress policy: {url}"));
            }
            let resp = http.get(url).send().await.map_err(|e| e.to_string())?;
            if !resp.status().is_success() {
                return Err(format!("spec fetch returned HTTP {}", resp.status().as_u16()));
            }
            resp.json::<Value>().await.map_err(|e| e.to_string())
        }
    }
}

/// Pick the API base URL: explicit override, else spec `servers[0].url`, else the
/// origin of the spec URL.
fn resolve_base_url(server: &StoredOpenApiServer, spec: &Value) -> String {
    if !server.base_url.is_empty() {
        return server.base_url.clone();
    }
    if let Some(url) = spec.get("servers").and_then(|s| s.as_array()).and_then(|a| a.first()).and_then(|s| s.get("url")).and_then(|u| u.as_str())
        && !url.is_empty()
    {
        return url.to_string();
    }
    if let SpecSource::Url(spec_url) = &server.spec
        && let Some(origin) = origin_of(spec_url)
    {
        return origin;
    }
    String::new()
}

/// Scheme + host (+ port) of a URL, e.g. `https://api.example.com`.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    if host.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host}"))
}

/// Walk `paths.<path>.<method>` and build one `ToolDef` + `Operation` per
/// operation, applying the include-filter and the per-connection cap.
fn parse_operations(
    conn: &str,
    spec: &Value,
    include: &[String],
) -> (Vec<ToolDef>, BTreeMap<String, Operation>) {
    const METHODS: [&str; 5] = ["get", "post", "put", "patch", "delete"];
    let mut defs = Vec::new();
    let mut ops = BTreeMap::new();

    let Some(paths) = spec.get("paths").and_then(|p| p.as_object()) else {
        return (defs, ops);
    };

    for (path, item) in paths {
        let Some(item) = item.as_object() else { continue };
        // Path-level parameters apply to every operation under this path.
        let path_level_params = item.get("parameters").and_then(|p| p.as_array()).cloned().unwrap_or_default();

        for method in METHODS {
            let Some(op_val) = item.get(method) else { continue };

            let op_id = op_val
                .get("operationId")
                .and_then(|v| v.as_str())
                .map(sanitize_name)
                .unwrap_or_else(|| sanitize_name(&format!("{method}_{path}")));

            if !include.is_empty() && !include.iter().any(|i| sanitize_name(i) == op_id) {
                continue;
            }
            if ops.len() >= MAX_OPS_PER_CONNECTION {
                warn!(name = %conn, cap = MAX_OPS_PER_CONNECTION, "OpenAPI operation cap reached — remaining operations dropped; narrow with `include`");
                return (defs, ops);
            }

            // Merge path-level + operation-level parameters.
            let mut params = path_level_params.clone();
            if let Some(op_params) = op_val.get("parameters").and_then(|p| p.as_array()) {
                params.extend(op_params.iter().cloned());
            }

            let (schema, op) = build_operation(spec, method, path, &params, op_val);

            let description = op_val
                .get("summary")
                .or_else(|| op_val.get("description"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            defs.push(ToolDef {
                name: format!("{conn}__{op_id}"),
                description,
                input_schema: schema,
                cache_control: None,
            });
            ops.insert(op_id, op);
        }
    }

    (defs, ops)
}

/// Build the synthesized object schema + `Operation` from one operation's
/// parameters and request body.
fn build_operation(
    spec: &Value,
    method: &str,
    path: &str,
    params: &[Value],
    op_val: &Value,
) -> (Value, Operation) {
    let mut properties = Map::new();
    let mut required = Vec::new();
    let mut path_params = Vec::new();
    let mut query_params = Vec::new();
    let mut header_params = Vec::new();

    for p in params {
        let Some(name) = p.get("name").and_then(|v| v.as_str()) else { continue };
        let location = p.get("in").and_then(|v| v.as_str()).unwrap_or("");
        let mut schema = p.get("schema").cloned().unwrap_or_else(|| json!({"type": "string"}));
        schema = resolve_refs(spec, schema, 0);
        if let Some(desc) = p.get("description").and_then(|v| v.as_str())
            && let Some(obj) = schema.as_object_mut()
        {
            obj.entry("description").or_insert_with(|| json!(desc));
        }
        properties.insert(name.to_string(), schema);

        let req = p.get("required").and_then(|v| v.as_bool()).unwrap_or(false);
        match location {
            "path" => {
                path_params.push(name.to_string());
                required.push(name.to_string()); // path params are always required
            }
            "query" => {
                query_params.push(name.to_string());
                if req {
                    required.push(name.to_string());
                }
            }
            "header" => {
                header_params.push(name.to_string());
                if req {
                    required.push(name.to_string());
                }
            }
            _ => {}
        }
    }

    // JSON request body → `body` property.
    let mut has_body = false;
    if let Some(body) = op_val.get("requestBody") {
        if let Some(schema) = body
            .get("content")
            .and_then(|c| c.get("application/json"))
            .and_then(|j| j.get("schema"))
        {
            let resolved = resolve_refs(spec, schema.clone(), 0);
            properties.insert("body".to_string(), resolved);
            has_body = true;
            if body.get("required").and_then(|v| v.as_bool()).unwrap_or(false) {
                required.push("body".to_string());
            }
        }
    }

    let schema = json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": required,
    });

    let op = Operation {
        method: method.to_uppercase(),
        path: path.to_string(),
        path_params,
        query_params,
        header_params,
        has_body,
    };
    (schema, op)
}

/// Recursively inline `#/components/...` `$ref`s so the model receives a concrete
/// schema. Bounded by [`MAX_REF_DEPTH`] to defend against cyclic specs.
fn resolve_refs(spec: &Value, schema: Value, depth: usize) -> Value {
    if depth >= MAX_REF_DEPTH {
        return schema;
    }
    match schema {
        Value::Object(mut obj) => {
            if let Some(Value::String(reference)) = obj.get("$ref").cloned() {
                if let Some(target) = resolve_pointer(spec, &reference) {
                    return resolve_refs(spec, target, depth + 1);
                }
                return Value::Object(obj);
            }
            for (_, v) in obj.iter_mut() {
                *v = resolve_refs(spec, v.take(), depth + 1);
            }
            Value::Object(obj)
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(|v| resolve_refs(spec, v, depth + 1)).collect()),
        other => other,
    }
}

/// Resolve a local JSON pointer like `#/components/schemas/Pet`.
fn resolve_pointer(spec: &Value, reference: &str) -> Option<Value> {
    let pointer = reference.strip_prefix('#')?;
    spec.pointer(pointer).cloned()
}

/// Sanitize a name to the tool-name charset accepted by providers
/// (`[A-Za-z0-9_-]`); other chars collapse to `_`.
fn sanitize_name(raw: &str) -> String {
    let mut out: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    fn petstore_spec(base: &str) -> Value {
        json!({
            "openapi": "3.0.0",
            "servers": [{ "url": base }],
            "components": {
                "schemas": {
                    "Pet": {
                        "type": "object",
                        "properties": { "name": { "type": "string" }, "tag": { "type": "string" } },
                        "required": ["name"]
                    }
                }
            },
            "paths": {
                "/pets/{petId}": {
                    "get": {
                        "operationId": "getPetById",
                        "summary": "Find pet by ID",
                        "parameters": [
                            { "name": "petId", "in": "path", "required": true, "schema": { "type": "string" } }
                        ]
                    }
                },
                "/pets": {
                    "get": {
                        "operationId": "listPets",
                        "parameters": [
                            { "name": "limit", "in": "query", "schema": { "type": "integer" } }
                        ]
                    },
                    "post": {
                        "operationId": "createPet",
                        "requestBody": {
                            "required": true,
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Pet" } } }
                        }
                    }
                }
            }
        })
    }

    #[test]
    fn parses_operations_with_synthesized_schema() {
        let spec = petstore_spec("https://api.example.com");
        let (defs, ops) = parse_operations("petstore", &spec, &[]);
        assert_eq!(ops.len(), 3);
        let names: Vec<_> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"petstore__getPetById"));
        assert!(names.contains(&"petstore__listPets"));
        assert!(names.contains(&"petstore__createPet"));

        // getPetById: petId is a required path param.
        let get = defs.iter().find(|d| d.name == "petstore__getPetById").unwrap();
        assert_eq!(get.input_schema["required"], json!(["petId"]));
        assert_eq!(get.input_schema["properties"]["petId"]["type"], "string");

        // createPet: body required, $ref inlined to the concrete Pet schema.
        let create = defs.iter().find(|d| d.name == "petstore__createPet").unwrap();
        assert_eq!(create.input_schema["required"], json!(["body"]));
        assert_eq!(create.input_schema["properties"]["body"]["properties"]["name"]["type"], "string");
        assert!(ops["createPet"].has_body);
    }

    #[test]
    fn include_filter_limits_operations() {
        let spec = petstore_spec("https://api.example.com");
        let (defs, ops) = parse_operations("petstore", &spec, &["listPets".to_string()]);
        assert_eq!(ops.len(), 1);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "petstore__listPets");
    }

    #[tokio::test]
    async fn executes_get_with_path_and_query() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(GET).path("/pets").query_param("limit", "5");
            then.status(200).body("[]");
        });

        let spec = petstore_spec(&server.base_url());
        let conn = StoredOpenApiServer {
            name: "petstore".into(),
            spec: SpecSource::Inline(spec),
            base_url: String::new(),
            auth: OpenApiAuth::None,
            include: vec![],
            timeout_secs: None,
        };
        let (defs, dispatch) =
            build_session_openapi(&reqwest::Client::new(), std::slice::from_ref(&conn), &EgressPolicy::default_safe()).await;
        assert!(defs.iter().any(|d| d.name == "petstore__listPets"));

        let (_, _, exec) = dispatch.iter().find(|(p, _, _)| p == "petstore__listPets").unwrap();
        let out = exec.call_tool("listPets", &json!({ "limit": 5 })).await.unwrap();
        m.assert();
        assert!(out.starts_with("HTTP 200"));
    }

    #[tokio::test]
    async fn executes_post_with_body_and_bearer_auth() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST).path("/pets").header("authorization", "Bearer secret").json_body(json!({ "name": "Rex" }));
            then.status(201).body("{\"id\":1}");
        });

        let spec = petstore_spec(&server.base_url());
        let conn = StoredOpenApiServer {
            name: "petstore".into(),
            spec: SpecSource::Inline(spec),
            base_url: String::new(),
            auth: OpenApiAuth::Bearer { token: "secret".into() },
            include: vec![],
            timeout_secs: None,
        };
        let (_, dispatch) =
            build_session_openapi(&reqwest::Client::new(), std::slice::from_ref(&conn), &EgressPolicy::default_safe()).await;
        let (_, _, exec) = dispatch.iter().find(|(p, _, _)| p == "petstore__createPet").unwrap();
        let out = exec.call_tool("createPet", &json!({ "body": { "name": "Rex" } })).await.unwrap();
        m.assert();
        assert!(out.starts_with("HTTP 201"));
    }

    #[tokio::test]
    async fn egress_policy_blocks_disallowed_host() {
        // Deny-all policy: a call must be refused before hitting the network.
        let spec = petstore_spec("https://api.blocked.example");
        let conn = StoredOpenApiServer {
            name: "petstore".into(),
            spec: SpecSource::Inline(spec),
            base_url: String::new(),
            auth: OpenApiAuth::None,
            include: vec!["listPets".into()],
            timeout_secs: None,
        };
        let deny = EgressPolicy { rules: vec![], default_action: crate::egress::EgressAction::Deny };
        let (_, dispatch) =
            build_session_openapi(&reqwest::Client::new(), std::slice::from_ref(&conn), &deny).await;
        let (_, _, exec) = dispatch.iter().find(|(p, _, _)| p == "petstore__listPets").unwrap();
        let err = exec.call_tool("listPets", &json!({})).await.unwrap_err();
        assert!(err.contains("egress policy"));
    }

    #[test]
    fn sanitize_collapses_invalid_chars() {
        assert_eq!(sanitize_name("get pet/by:id"), "get_pet_by_id");
        assert_eq!(sanitize_name("__weird__"), "weird");
    }
}
