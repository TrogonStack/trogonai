use serde::Serialize;
use serde_json::Value;

use crate::{JsonObject, ProtocolError};

/// One hook matcher of the initialize request. TypeScript leaves `matcher` out when it is unset; Python sends
/// `null`.
#[derive(Serialize)]
pub(crate) struct HookMatcherPayload<'a> {
    /// `None` leaves the key out, `Some(None)` sends `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<Option<&'a str>>,
    #[serde(rename = "hookCallbackIds")]
    pub callback_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<serde_json::Number>,
    #[serde(rename = "onFailure", skip_serializing_if = "Option::is_none")]
    pub on_failure: Option<&'static str>,
}

/// The hooks section: events in the order they first appear among the registrations, as both SDKs keep the
/// insertion order of their hooks object.
pub(crate) type HooksPayload<'a> = indexmap::IndexMap<&'static str, Vec<HookMatcherPayload<'a>>>;

/// TypeScript `buildInitializeRequest`: every key in upstream order, an unset key left out.
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InitializeRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hooks: Option<HooksPayload<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sdk_mcp_servers: Option<Vec<&'a str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sdk_mcp_server_configs: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sdk_mcp_server_manifests: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<&'a JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<Vec<&'a str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_mode_instructions: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt_snapshot: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append_subagent_system_prompt: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_aliases: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_running_mcp_tools: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_dynamic_sections: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agents: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_search_isolation_exempt_mcp_servers: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_suggestions: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_progress_summaries: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_subagent_text: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_dialog_kinds: Option<Vec<&'a str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_task_stop_affordance: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rapid_followup_preempt: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_nested_subagent_tool_calls: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_trust: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<crate::plugin::PluginPayload<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub managed_settings: Option<Value>,
}

/// Python `Query.initialize`: `hooks` always present (`null` when there are none), then only the keys Python
/// knows, in its order. Python sends `forwardSubagentText` only when it is `true`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PythonInitializeRequest<'a> {
    pub hooks: Option<HooksPayload<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agents: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_dynamic_sections: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt_snapshot: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_subagent_text: Option<Value>,
}

impl<'a> InitializeRequest<'a> {
    /// The request in the given dialect's shape and key order.
    pub(crate) fn into_payload(self, style: crate::ArgvStyle) -> RequestPayload<'a> {
        match style {
            crate::ArgvStyle::TypeScript => RequestPayload::Initialize(Box::new(self)),
            crate::ArgvStyle::Python => RequestPayload::PythonInitialize(Box::new(PythonInitializeRequest {
                hooks: self.hooks,
                agents: self.agents,
                exclude_dynamic_sections: self.exclude_dynamic_sections,
                system_prompt_snapshot: self.system_prompt_snapshot,
                skills: self.skills,
                forward_subagent_text: self.forward_subagent_text.filter(|value| *value == Value::Bool(true)),
            })),
        }
    }
}

/// The `mcp_message` request `sdk.mjs` `sendMcpServerMessageToCli` writes: `{type, request_id, request}`, a
/// different key order from its other control requests.
#[derive(Serialize)]
pub(crate) struct TypeScriptMcpForward<'a, T> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub request_id: &'a str,
    pub request: T,
}

#[derive(Serialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub(crate) enum RequestPayload<'a> {
    Initialize(Box<InitializeRequest<'a>>),
    #[serde(rename = "initialize")]
    PythonInitialize(Box<PythonInitializeRequest<'a>>),
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
    McpMessage {
        server_name: &'a str,
        message: &'a JsonObject,
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
    /// Python `{"type", "request_id", "request"}`.
    #[serde(rename = "control_request")]
    Request { request_id: &'a str, request: T },
    #[serde(rename = "control_response")]
    Response { response: CallbackResponse<'a, T> },
}

/// TypeScript `{request_id, type: "control_request", request}`.
#[derive(Serialize)]
pub(crate) struct TypeScriptControlRequest<'a, T> {
    pub request_id: &'a str,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub request: T,
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
