use crate::{
    CallbackError, CancellationToken, JsonObject, McpServerDialect, McpServerName, McpToolInputSchema, SdkTools,
};
use serde_json::Value;
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};

#[path = "sdk_mcp_python.rs"]
mod python;
#[path = "sdk_mcp_typescript.rs"]
mod typescript;

#[derive(Debug, thiserror::Error)]
pub enum McpServerError {
    #[error("MCP callback failed")]
    Callback(#[from] CallbackError),
    #[error("MCP request is malformed")]
    Json(#[from] serde_json::Error),
    /// A message the Python dialect cannot read as a JSON-RPC request or notification, or a request whose id is
    /// already in flight. The Python server reports these to its transport instead of answering them.
    #[error("MCP request is invalid: {0}")]
    Protocol(String),
}

pub type McpFuture<'a> = Pin<Box<dyn Future<Output = Result<Option<JsonObject>, McpServerError>> + Send + 'a>>;

pub trait SdkMcpServer: Send + Sync {
    fn open_session(&self) -> Arc<dyn SdkMcpSession>;
}

pub trait SdkMcpSession: Send + Sync {
    fn request(&self, message: JsonObject, signal: CancellationToken) -> McpFuture<'_>;
}

/// An in-process MCP server like the TypeScript `createSdkMcpServer` and the Python `create_sdk_mcp_server`.
#[derive(Clone)]
pub struct LocalMcpServerOptions {
    pub name: McpServerName,
    /// `serverInfo.version`; both SDKs answer `1.0.0` when it is omitted.
    pub version: Option<String>,
    /// `None` registers no tool handlers, like omitting `tools` in TypeScript.
    pub tools: Option<SdkTools>,
    /// The TypeScript `instructions`, answered from `initialize` when not empty. The Python dialect ignores it:
    /// `create_sdk_mcp_server(name, version, tools)` in the Python SDK takes no instructions.
    pub instructions: Option<String>,
    /// The TypeScript `alwaysLoad`: every listed tool carries `_meta["anthropic/alwaysLoad"]`. The Python dialect
    /// ignores it: `create_sdk_mcp_server(name, version, tools)` in the Python SDK takes no such option.
    pub always_load: bool,
    pub dialect: McpServerDialect,
}

impl LocalMcpServerOptions {
    /// A TypeScript server with no tools, version, instructions or `alwaysLoad`.
    pub fn new(name: McpServerName) -> Self {
        Self {
            name,
            version: None,
            tools: None,
            instructions: None,
            always_load: false,
            dialect: McpServerDialect::default(),
        }
    }
}

impl std::fmt::Debug for LocalMcpServerOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalMcpServerOptions")
            .field("name", &self.name)
            .field("tool_count", &self.tools.as_ref().map_or(0, SdkTools::len))
            .field("dialect", &self.dialect)
            .finish_non_exhaustive()
    }
}

pub struct LocalMcpServer {
    state: Arc<ServerState>,
}

pub(crate) struct ServerState {
    pub(crate) name: McpServerName,
    pub(crate) version: Option<String>,
    pub(crate) tools: Option<SdkTools>,
    pub(crate) instructions: Option<String>,
    pub(crate) always_load: bool,
    pub(crate) dialect: McpServerDialect,
    /// The Python dialect lists and checks each name against the schema its last definition builds.
    pub(crate) python_schemas: HashMap<String, McpToolInputSchema>,
}

impl ServerState {
    pub(crate) fn version(&self) -> &str {
        self.version.as_deref().unwrap_or("1.0.0")
    }
}

pub(crate) struct LocalMcpSession {
    pub(crate) state: Arc<ServerState>,
    pub(crate) initialized: AtomicBool,
    /// Set once a Python handler ends the server's `run()`, until the next `initialize` starts it again.
    pub(crate) stopped: AtomicBool,
    in_flight: Mutex<HashMap<String, CancellationToken>>,
}

/// A JSON-RPC request id as the response echoes it, and the key that a `notifications/cancelled` names it by.
#[derive(Clone, Debug)]
pub(crate) struct RequestId {
    pub(crate) value: Value,
    pub(crate) key: String,
}

impl RequestId {
    pub(crate) fn text(text: &str) -> Self {
        Self {
            value: Value::String(text.to_owned()),
            key: format!("s:{text}"),
        }
    }

    pub(crate) fn number(number: i64) -> Self {
        Self {
            value: Value::from(number),
            key: format!("n:{number}"),
        }
    }
}

/// Removes a request from the in-flight table when it finishes or its future is dropped.
pub(crate) struct InFlight<'a> {
    session: &'a LocalMcpSession,
    key: String,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.session.table().remove(&self.key);
    }
}

impl LocalMcpSession {
    fn table(&self) -> std::sync::MutexGuard<'_, HashMap<String, CancellationToken>> {
        self.in_flight.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Registers a request, replacing any request with the same id like the TypeScript `Protocol`.
    pub(crate) fn track(&self, id: &RequestId, token: CancellationToken) -> InFlight<'_> {
        self.table().insert(id.key.clone(), token);
        InFlight {
            session: self,
            key: id.key.clone(),
        }
    }

    /// Registers a request unless one with the same id is in flight, like the Python `ServerSession`.
    pub(crate) fn track_new(&self, id: &RequestId, token: CancellationToken) -> Option<InFlight<'_>> {
        let mut table = self.table();
        if table.contains_key(&id.key) {
            return None;
        }
        table.insert(id.key.clone(), token);
        Some(InFlight {
            session: self,
            key: id.key.clone(),
        })
    }

    pub(crate) fn cancel(&self, key: &str) {
        if let Some(token) = self.table().get(key) {
            token.cancel();
        }
    }

    /// Ends the session like the Python server's `run()` returning: every request in flight is abandoned.
    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        for token in self.table().values() {
            token.cancel();
        }
    }
}

impl From<LocalMcpServerOptions> for LocalMcpServer {
    fn from(options: LocalMcpServerOptions) -> Self {
        let LocalMcpServerOptions {
            name,
            version,
            tools,
            instructions,
            always_load,
            dialect,
        } = options;
        let mut python_schemas = HashMap::new();
        for tool in tools.iter().flat_map(|tools| tools.values()) {
            if dialect == McpServerDialect::TypeScript {
                for line in crate::mcp_tool_name_warning::tool_name_warning_lines(tool.name().as_str()) {
                    // One line on purpose: with the `log` feature of `tracing` enabled by another workspace
                    // member, the macro repeats each field expression for the log fallback, and the
                    // fallback copy on a line of its own is never executed, which reads as uncovered.
                    tracing::warn!(server = %name.as_str(), "{line}");
                }
            }
            if dialect == McpServerDialect::Python {
                let schema = python::input_schema(tool.input_schema().as_json());
                python_schemas.insert(tool.name().as_str().to_owned(), McpToolInputSchema::new(schema));
            }
        }
        Self {
            state: Arc::new(ServerState {
                name,
                version,
                tools,
                instructions,
                always_load,
                dialect,
                python_schemas,
            }),
        }
    }
}

/// `{"content":[{"type":"text","text":message}],"isError":true}`, the result both SDKs answer a failed call with.
pub(crate) fn tool_error(message: String) -> Value {
    let content = JsonObject::from_iter([("type".into(), "text".into()), ("text".into(), message.into())]);
    Value::Object(JsonObject::from_iter([
        ("content".into(), Value::Array(vec![Value::Object(content)])),
        ("isError".into(), true.into()),
    ]))
}

impl SdkMcpServer for LocalMcpServer {
    fn open_session(&self) -> Arc<dyn SdkMcpSession> {
        Arc::new(LocalMcpSession {
            state: self.state.clone(),
            initialized: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            in_flight: Mutex::new(HashMap::new()),
        })
    }
}

impl SdkMcpSession for LocalMcpSession {
    fn request(&self, message: JsonObject, signal: CancellationToken) -> McpFuture<'_> {
        match self.state.dialect {
            McpServerDialect::TypeScript => Box::pin(typescript::request(self, message, signal)),
            McpServerDialect::Python => Box::pin(python::request(self, message, signal)),
        }
    }
}

impl std::fmt::Debug for LocalMcpServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalMcpServer")
            .field("name", &self.state.name)
            .field("tool_count", &self.state.tools.as_ref().map_or(0, SdkTools::len))
            .field("dialect", &self.state.dialect)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[path = "sdk_mcp_server_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "sdk_mcp_dialect_tests.rs"]
mod dialect_tests;

impl std::fmt::Debug for dyn SdkMcpServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("SdkMcpServer").finish_non_exhaustive()
    }
}
