use crate::{
    CallbackError, CancellationToken, JsonObject, McpServerName, McpToolRequest, SdkTools,
    constants::SUPPORTED_PROTOCOL_VERSIONS,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{future::Future, pin::Pin, sync::Arc};

#[derive(Debug, thiserror::Error)]
pub enum McpServerError {
    #[error("MCP callback failed")]
    Callback(#[from] CallbackError),
    #[error("MCP request is malformed")]
    Json(#[from] serde_json::Error),
}

pub type McpFuture<'a> = Pin<Box<dyn Future<Output = Result<Option<JsonObject>, McpServerError>> + Send + 'a>>;

pub trait SdkMcpServer: Send + Sync {
    fn open_session(&self) -> Arc<dyn SdkMcpSession>;
}

pub trait SdkMcpSession: Send + Sync {
    fn request(&self, message: JsonObject, signal: CancellationToken) -> McpFuture<'_>;
}

#[derive(Clone)]
pub struct LocalMcpServerOptions {
    pub name: McpServerName,
    pub version: String,
    pub tools: SdkTools,
}

impl std::fmt::Debug for LocalMcpServerOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalMcpServerOptions")
            .field("name", &self.name)
            .field("tool_count", &self.tools.len())
            .finish_non_exhaustive()
    }
}

pub struct LocalMcpServer {
    name: McpServerName,
    version: String,
    tools: SdkTools,
}

struct LocalMcpSession {
    name: McpServerName,
    version: String,
    tools: SdkTools,
}

impl From<LocalMcpServerOptions> for LocalMcpServer {
    fn from(options: LocalMcpServerOptions) -> Self {
        let LocalMcpServerOptions { name, version, tools } = options;
        for tool in tools.values() {
            if !is_valid_tool_name(tool.name().as_str()) {
                // One line on purpose: with the `log` feature of `tracing` enabled by another workspace
                // member, the macro repeats each field expression for the log fallback, and the
                // fallback copy on a line of its own is never executed, which reads as uncovered.
                tracing::warn!(server = %name.as_str(), tool = %tool.name(), "MCP tool names should be 1 to 128 ASCII letters, digits, underscores, hyphens or periods; tool registration will proceed, but this may cause compatibility issues");
            }
        }
        Self { name, version, tools }
    }
}

#[derive(Deserialize)]
struct RpcRequestWire {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<JsonObject>,
}

#[derive(Deserialize)]
struct ToolCallWire {
    name: String,
    #[serde(default)]
    arguments: JsonObject,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitializeWire {
    protocol_version: String,
    #[serde(rename = "capabilities")]
    _capabilities: JsonObject,
    #[serde(rename = "clientInfo")]
    _client_info: ClientInfoWire,
}

#[derive(Deserialize)]
struct ClientInfoWire {
    #[serde(rename = "name")]
    _name: String,
    #[serde(rename = "version")]
    _version: String,
}

#[derive(Serialize)]
struct EmptyWire {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InitializeResultWire<'a> {
    protocol_version: &'a str,
    capabilities: CapabilitiesWire,
    server_info: ServerInfoWire<'a>,
}

#[derive(Serialize)]
struct CapabilitiesWire {
    tools: EmptyWire,
}

#[derive(Serialize)]
struct ServerInfoWire<'a> {
    name: &'a str,
    version: &'a str,
}

fn rpc_result(id: Value, result: Value) -> JsonObject {
    JsonObject::from_iter([
        ("jsonrpc".into(), "2.0".into()),
        ("id".into(), id),
        ("result".into(), result),
    ])
}

fn tool_error(message: String) -> Value {
    let content = JsonObject::from_iter([("type".into(), "text".into()), ("text".into(), message.into())]);
    Value::Object(JsonObject::from_iter([
        ("content".into(), Value::Array(vec![Value::Object(content)])),
        ("isError".into(), true.into()),
    ]))
}

fn rpc_error(id: Value, code: i32, message: &'static str) -> JsonObject {
    let error = JsonObject::from_iter([("code".into(), code.into()), ("message".into(), message.into())]);
    JsonObject::from_iter([
        ("jsonrpc".into(), "2.0".into()),
        ("id".into(), id),
        ("error".into(), error.into()),
    ])
}

impl SdkMcpServer for LocalMcpServer {
    fn open_session(&self) -> Arc<dyn SdkMcpSession> {
        Arc::new(LocalMcpSession {
            name: self.name.clone(),
            version: self.version.clone(),
            tools: self.tools.clone(),
        })
    }
}

impl SdkMcpSession for LocalMcpSession {
    fn request(&self, message: JsonObject, signal: CancellationToken) -> McpFuture<'_> {
        Box::pin(async move {
            let request: RpcRequestWire = serde_json::from_value(message.into())?;
            if request.jsonrpc != "2.0" {
                return Ok(Some(rpc_error(
                    request.id.unwrap_or(Value::Null),
                    -32600,
                    "Invalid JSON-RPC version",
                )));
            }
            let Some(id) = request.id else {
                return Ok(None);
            };
            let valid_id = match &id {
                Value::String(_) => true,
                Value::Number(number) => number.is_i64() || number.is_u64(),
                _ => false,
            };
            if !valid_id {
                return Ok(Some(rpc_error(Value::Null, -32600, "Invalid request identifier")));
            }
            if signal.is_cancelled() {
                return Err(CallbackError::Cancelled.into());
            }
            let result = match request.method.as_str() {
                "initialize" => {
                    let initialize: InitializeWire =
                        match serde_json::from_value(request.params.unwrap_or_default().into()) {
                            Ok(value) => value,
                            Err(_) => return Ok(Some(rpc_error(id, -32602, "Invalid initialize parameters"))),
                        };
                    let requested = initialize.protocol_version.as_str();
                    let version = SUPPORTED_PROTOCOL_VERSIONS
                        .into_iter()
                        .find(|supported| *supported == requested)
                        .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0]);
                    serde_json::to_value(InitializeResultWire {
                        protocol_version: version,
                        capabilities: CapabilitiesWire { tools: EmptyWire {} },
                        server_info: ServerInfoWire {
                            name: self.name.as_str(),
                            version: &self.version,
                        },
                    })?
                }
                "ping" => serde_json::to_value(EmptyWire {})?,
                "tools/list" => {
                    let tools = self
                        .tools
                        .values()
                        .map(|tool| {
                            Value::Object(JsonObject::from_iter([
                                ("name".into(), tool.name().as_str().into()),
                                ("description".into(), tool.description().into()),
                                ("inputSchema".into(), tool.input_schema().as_json().clone().into()),
                            ]))
                        })
                        .collect();
                    JsonObject::from_iter([("tools".into(), Value::Array(tools))]).into()
                }
                "tools/call" => {
                    let call: ToolCallWire = match serde_json::from_value(request.params.unwrap_or_default().into()) {
                        Ok(value) => value,
                        Err(_) => return Ok(Some(rpc_error(id, -32602, "Invalid tool call"))),
                    };
                    let Some(tool) = self.tools.get(&call.name) else {
                        return Ok(Some(rpc_result(
                            id,
                            tool_error(format!("Tool {} not found", call.name)),
                        )));
                    };
                    if let Some(error) = tool
                        .input_schema()
                        .validation_error(&Value::Object(call.arguments.clone()))
                    {
                        return Ok(Some(rpc_result(
                            id,
                            tool_error(format!(
                                "Input validation error: Invalid arguments for tool {}: {error}",
                                call.name
                            )),
                        )));
                    }
                    match tool
                        .call(McpToolRequest {
                            arguments: call.arguments,
                            signal,
                        })
                        .await
                    {
                        Ok(output) => output.to_wire_json()?,
                        Err(CallbackError::Cancelled) => return Err(CallbackError::Cancelled.into()),
                        Err(CallbackError::Failed(error)) => tool_error(error.to_string()),
                    }
                }
                _ => return Ok(Some(rpc_error(id, -32601, "Method not found"))),
            };
            Ok(Some(rpc_result(id, result)))
        })
    }
}

impl std::fmt::Debug for LocalMcpServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalMcpServer")
            .field("name", &self.name)
            .field("tool_count", &self.tools.len())
            .finish_non_exhaustive()
    }
}

/// MCP tool names are one to 128 ASCII letters, digits, underscores, hyphens or periods.
fn is_valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

#[cfg(test)]
#[path = "sdk_mcp_server_tests.rs"]
mod tests;

impl std::fmt::Debug for dyn SdkMcpServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("SdkMcpServer").finish_non_exhaustive()
    }
}
