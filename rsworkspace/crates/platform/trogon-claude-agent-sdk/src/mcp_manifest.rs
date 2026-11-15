//! `sdk.mjs` `captureSdkMcpServerManifests`: before the first `initialize` request, each in-process MCP server is
//! handshaken once so the request can carry what the CLI would otherwise ask for over the control channel.

use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{mpsc::UnboundedSender, oneshot};

use super::{McpTransportEvent, WaiterTable};
use crate::{CancellationToken, JsonObject, SdkMcpSession, constants};

fn object<const N: usize>(entries: [(&str, Value); N]) -> JsonObject {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn text(value: &str) -> Value {
    Value::String(value.to_owned())
}

/// The `initialize` request, with `M8` (no capabilities) and `{...N8, version}` as the client.
pub(crate) fn initialize_message() -> JsonObject {
    let client = object([
        ("name", text("claude-code")),
        ("title", text("Claude Code")),
        ("description", text("Anthropic's agentic coding tool")),
        ("websiteUrl", text("https://claude.com/claude-code")),
        ("version", text("0.3.323")),
    ]);
    let params = object([
        (
            "protocolVersion",
            text(constants::MCP_MANIFEST_CAPTURE_PROTOCOL_VERSION),
        ),
        ("capabilities", Value::Object(JsonObject::new())),
        ("clientInfo", Value::Object(client)),
    ]);
    object([
        ("jsonrpc", text("2.0")),
        (
            "id",
            text(&format!("{}initialize", constants::MCP_MANIFEST_CAPTURE_ID_PREFIX)),
        ),
        ("method", text("initialize")),
        ("params", Value::Object(params)),
    ])
}

pub(crate) fn initialized_notification() -> JsonObject {
    object([("jsonrpc", text("2.0")), ("method", text("notifications/initialized"))])
}

pub(crate) fn tools_list_message() -> JsonObject {
    object([
        ("jsonrpc", text("2.0")),
        (
            "id",
            text(&format!("{}tools-list", constants::MCP_MANIFEST_CAPTURE_ID_PREFIX)),
        ),
        ("method", text("tools/list")),
    ])
}

/// One server's handshake, run beside the others while the driver routes what their transports send.
pub(crate) struct Capture {
    pub name: String,
    pub session: Arc<dyn SdkMcpSession>,
    pub waiters: WaiterTable,
    pub outbound: UnboundedSender<McpTransportEvent>,
}

impl Capture {
    /// `handleMcpControlRequest`: registers the waiter under `${server}:${id}`, hands the message to the server and
    /// waits for the answer its transport sends.
    async fn call(&self, message: JsonObject) -> Result<JsonObject, String> {
        let key = format!("{}:{}", self.name, crate::mcp_routing::js_string(message.get("id")));
        let (waiter, answer) = oneshot::channel();
        self.waiters.insert(key.clone(), waiter);
        match self.session.request(message, CancellationToken::new()).await {
            Ok(Some(reply)) => {
                let _ = self.outbound.send(McpTransportEvent::Message {
                    server: self.name.clone(),
                    message: reply,
                });
            }
            Ok(None) => {}
            Err(error) => {
                self.waiters.remove(&key);
                return Err(error.to_string());
            }
        }
        match answer.await {
            Ok(Ok(answer)) => Ok(answer),
            _ => Err("the server could not answer".to_owned()),
        }
    }

    /// The manifest `{initializeResult, toolsListResult?}`, or `None` when the CLI is left to handshake the server.
    pub(crate) async fn run(self) -> Option<(String, JsonObject)> {
        let name = self.name.clone();
        let answer = match self.call(initialize_message()).await {
            Ok(answer) => answer,
            Err(error) => {
                tracing::warn!(server = %name, %error, "the CLI will handshake it over the control channel");
                return None;
            }
        };
        let Some(initialized) = answer.get("result").cloned() else {
            tracing::warn!(server = %name, "answered initialize with an error; leaving it to the CLI");
            return None;
        };
        // `f.onmessage?.(notifications/initialized)`: what the server sends back goes the way of any message.
        if let Ok(Some(reply)) = self
            .session
            .request(initialized_notification(), CancellationToken::new())
            .await
        {
            let _ = self.outbound.send(McpTransportEvent::Message {
                server: name.clone(),
                message: reply,
            });
        }
        // Reading `capabilities` of `null` throws in `sdk.mjs`.
        if initialized.is_null() {
            tracing::warn!(server = %name, "the CLI will handshake it over the control channel");
            return None;
        }
        let has_tools = initialized
            .get("capabilities")
            .and_then(Value::as_object)
            .and_then(|capabilities| capabilities.get("tools"))
            .is_some_and(|tools| !tools.is_null());
        let mut tools = None;
        if has_tools {
            let answer = match self.call(tools_list_message()).await {
                Ok(answer) => answer,
                Err(error) => {
                    tracing::warn!(server = %name, %error, "the CLI will handshake it over the control channel");
                    return None;
                }
            };
            let Some(result) = answer.get("result") else {
                tracing::warn!(server = %name, "answered tools/list with an error; leaving it to the CLI");
                return None;
            };
            match result {
                Value::Null => return None,
                // A page with a `nextCursor` is not the whole list, so the CLI asks for it.
                Value::Object(page) if page.contains_key("nextCursor") => {}
                other => tools = Some(other.clone()),
            }
        }
        let mut manifest = JsonObject::new();
        manifest.insert("initializeResult".to_owned(), initialized);
        if let Some(tools) = tools {
            manifest.insert("toolsListResult".to_owned(), tools);
        }
        Some((name, manifest))
    }
}
