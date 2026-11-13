use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::{JsonObject, ProtocolError};

#[derive(Serialize)]
pub(crate) struct HookMatcherPayload<'a> {
    pub matcher: Option<&'a str>,
    #[serde(rename = "hookCallbackIds")]
    pub callback_ids: Vec<String>,
    #[serde(rename = "onFailure", skip_serializing_if = "Option::is_none")]
    pub on_failure: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<f64>,
}

#[derive(Serialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub(crate) enum RequestPayload<'a> {
    Initialize {
        hooks: Option<BTreeMap<String, Vec<HookMatcherPayload<'a>>>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        agents: Option<JsonObject>,
        #[serde(rename = "supportedDialogKinds", skip_serializing_if = "Option::is_none")]
        supported_dialog_kinds: Option<Vec<&'a str>>,
        #[serde(rename = "systemPrompt", skip_serializing_if = "Option::is_none")]
        system_prompt: Option<Vec<&'a str>>,
        #[serde(rename = "appendSystemPrompt", skip_serializing_if = "Option::is_none")]
        append_system_prompt: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        plugins: Option<Vec<crate::plugin::PluginPayload<'a>>>,
    },
    Interrupt {
        #[serde(skip_serializing_if = "Option::is_none")]
        cancel_queued: Option<bool>,
    },
    SetModel {
        model: Option<&'a str>,
    },
    SetPermissionMode {
        mode: &'a str,
    },
    RewindFiles {
        user_message_id: &'a str,
        dry_run: bool,
    },
    McpStatus,
    GetContextUsage,
    McpReconnect {
        #[serde(rename = "serverName")]
        server_name: &'a str,
    },
    McpToggle {
        #[serde(rename = "serverName")]
        server_name: &'a str,
        enabled: bool,
    },
    StopTask {
        task_id: &'a str,
    },
    BackgroundTasks {
        tool_use_id: Option<&'a str>,
    },
    McpSetServers {
        servers: JsonObject,
    },
}

impl RequestPayload<'_> {
    pub(crate) fn to_value(&self) -> Result<Value, ProtocolError> {
        serde_json::to_value(self).map_err(ProtocolError::from)
    }
}

#[derive(Serialize)]
#[serde(tag = "type")]
pub(crate) enum OutboundFrame<'a, T> {
    #[serde(rename = "control_request")]
    Request { request_id: &'a str, request: T },
    #[serde(rename = "control_response")]
    Response { response: CallbackResponse<'a, T> },
}

#[derive(Serialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub(crate) enum CallbackResponse<'a, T> {
    Success { request_id: &'a str, response: T },
    Error { request_id: &'a str, error: &'a str },
}

#[derive(Serialize)]
#[serde(tag = "behavior", rename_all = "snake_case")]
pub(crate) enum PermissionResponsePayload {
    Allow {
        #[serde(rename = "updatedInput")]
        updated_input: JsonObject,
        #[serde(rename = "updatedPermissions", skip_serializing_if = "Option::is_none")]
        updated_permissions: Option<Vec<Value>>,
    },
    Deny {
        message: String,
        interrupt: bool,
    },
}

#[derive(Serialize)]
#[serde(tag = "type")]
pub(crate) enum EmbeddedMcpPayload<'a> {
    #[serde(rename = "sdk")]
    Sdk { name: &'a str },
}

impl EmbeddedMcpPayload<'_> {
    pub(crate) fn to_value(&self) -> Result<Value, ProtocolError> {
        serde_json::to_value(self).map_err(ProtocolError::from)
    }
}

#[derive(Serialize)]
pub(crate) struct McpConfigPayload {
    #[serde(rename = "mcpServers")]
    pub servers: JsonObject,
}

#[derive(Serialize)]
pub(crate) struct McpResponsePayload {
    pub mcp_response: Value,
}

#[derive(Serialize)]
struct EmptyResult {}

#[derive(Serialize)]
struct RpcAcknowledgment {
    jsonrpc: &'static str,
    result: EmptyResult,
}

impl McpResponsePayload {
    pub(crate) fn new(response: Option<JsonObject>) -> Result<Self, ProtocolError> {
        let mcp_response = match response {
            Some(response) => Value::Object(response),
            None => serde_json::to_value(RpcAcknowledgment {
                jsonrpc: "2.0",
                result: EmptyResult {},
            })?,
        };
        Ok(Self { mcp_response })
    }
}
