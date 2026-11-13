use crate::upstream::hooks::{HookInput, HookJSONOutput};
use crate::{CancellationToken, JsonObject, Timeout, ToolUseId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{future::Future, pin::Pin, sync::Arc};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    PostToolBatch,
    Notification,
    UserPromptSubmit,
    UserPromptExpansion,
    SessionStart,
    SessionEnd,
    Stop,
    StopFailure,
    SubagentStart,
    SubagentStop,
    PreCompact,
    PostCompact,
    PreModelSwitch,
    PostModelSwitch,
    PermissionRequest,
    PermissionDenied,
    Setup,
    TeammateIdle,
    TaskCreated,
    TaskCompleted,
    Elicitation,
    ElicitationResult,
    ConfigChange,
    WorktreeCreate,
    WorktreeRemove,
    InstructionsLoaded,
    CwdChanged,
    FileChanged,
    DirectoryAdded,
    MessageDisplay,
}

impl HookEvent {
    /// Every event, in the order upstream `HOOK_EVENTS` lists them.
    pub const ALL: [Self; 33] = [
        Self::PreToolUse,
        Self::PostToolUse,
        Self::PostToolUseFailure,
        Self::PostToolBatch,
        Self::Notification,
        Self::UserPromptSubmit,
        Self::UserPromptExpansion,
        Self::SessionStart,
        Self::SessionEnd,
        Self::Stop,
        Self::StopFailure,
        Self::SubagentStart,
        Self::SubagentStop,
        Self::PreCompact,
        Self::PostCompact,
        Self::PreModelSwitch,
        Self::PostModelSwitch,
        Self::PermissionRequest,
        Self::PermissionDenied,
        Self::Setup,
        Self::TeammateIdle,
        Self::TaskCreated,
        Self::TaskCompleted,
        Self::Elicitation,
        Self::ElicitationResult,
        Self::ConfigChange,
        Self::WorktreeCreate,
        Self::WorktreeRemove,
        Self::InstructionsLoaded,
        Self::CwdChanged,
        Self::FileChanged,
        Self::DirectoryAdded,
        Self::MessageDisplay,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::PostToolBatch => "PostToolBatch",
            Self::Notification => "Notification",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::UserPromptExpansion => "UserPromptExpansion",
            Self::SessionStart => "SessionStart",
            Self::SessionEnd => "SessionEnd",
            Self::Stop => "Stop",
            Self::StopFailure => "StopFailure",
            Self::SubagentStart => "SubagentStart",
            Self::SubagentStop => "SubagentStop",
            Self::PreCompact => "PreCompact",
            Self::PostCompact => "PostCompact",
            Self::PreModelSwitch => "PreModelSwitch",
            Self::PostModelSwitch => "PostModelSwitch",
            Self::PermissionRequest => "PermissionRequest",
            Self::PermissionDenied => "PermissionDenied",
            Self::Setup => "Setup",
            Self::TeammateIdle => "TeammateIdle",
            Self::TaskCreated => "TaskCreated",
            Self::TaskCompleted => "TaskCompleted",
            Self::Elicitation => "Elicitation",
            Self::ElicitationResult => "ElicitationResult",
            Self::ConfigChange => "ConfigChange",
            Self::WorktreeCreate => "WorktreeCreate",
            Self::WorktreeRemove => "WorktreeRemove",
            Self::InstructionsLoaded => "InstructionsLoaded",
            Self::CwdChanged => "CwdChanged",
            Self::FileChanged => "FileChanged",
            Self::DirectoryAdded => "DirectoryAdded",
            Self::MessageDisplay => "MessageDisplay",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HookFailureMode {
    #[default]
    Continue,
    Block,
}

impl HookFailureMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Block => "block",
        }
    }
}

#[derive(Clone, Debug)]
pub struct HookRequest {
    pub input: JsonObject,
    pub tool_use_id: Option<ToolUseId>,
    pub signal: CancellationToken,
}

impl HookRequest {
    /// The input as the upstream `HookInput` union. A payload no declared event reproduces
    /// exactly is `HookInput::Unknown` with every key kept.
    pub fn typed_input(&self) -> HookInput {
        let value = Value::Object(self.input.clone());
        match HookInput::best_match(&value) {
            Some((_, input)) => input,
            None => HookInput::Unknown(value),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CallbackError {
    #[error("callback failed")]
    Failed(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("callback was cancelled")]
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HookContinuation {
    #[default]
    Continue,
    Stop,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HookOutputVisibility {
    #[default]
    Visible,
    Suppressed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HookPermissionDecision {
    Allow,
    Deny,
    Ask,
    Defer,
}

impl HookPermissionDecision {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Ask => "ask",
            Self::Defer => "defer",
        }
    }
}

#[derive(Clone, Debug)]
pub enum HookSpecificOutput {
    PreToolUse {
        permission_decision: Option<HookPermissionDecision>,
        permission_decision_reason: Option<String>,
        updated_input: Option<JsonObject>,
        additional_context: Option<String>,
    },
    AdditionalContext {
        event: HookEvent,
        context: String,
    },
    Event {
        event: HookEvent,
        fields: JsonObject,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PreToolUseOutputWire<'a> {
    hook_event_name: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    permission_decision: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    permission_decision_reason: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_input: Option<&'a JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    additional_context: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextOutputWire<'a> {
    hook_event_name: &'static str,
    additional_context: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventOutputWire<'a> {
    hook_event_name: &'static str,
    #[serde(flatten)]
    fields: &'a JsonObject,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AsyncHookOutputWire {
    #[serde(rename = "async")]
    asynchronous: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    async_timeout: Option<u128>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncHookOutputWire<'a> {
    #[serde(rename = "continue", skip_serializing_if = "Option::is_none")]
    continuation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    suppress_output: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_reason: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decision: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_message: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hook_specific_output: Option<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HookRegistrationWire<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    matcher: Option<&'a str>,
    hook_callback_ids: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    on_failure: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<f64>,
}

impl HookSpecificOutput {
    fn to_wire_json(&self) -> Result<Value, serde_json::Error> {
        match self {
            Self::PreToolUse {
                permission_decision,
                permission_decision_reason,
                updated_input,
                additional_context,
            } => serde_json::to_value(PreToolUseOutputWire {
                hook_event_name: "PreToolUse",
                permission_decision: permission_decision.map(HookPermissionDecision::as_str),
                permission_decision_reason: permission_decision_reason.as_deref(),
                updated_input: updated_input.as_ref(),
                additional_context: additional_context.as_deref(),
            }),
            Self::AdditionalContext { event, context } => serde_json::to_value(ContextOutputWire {
                hook_event_name: event.as_str(),
                additional_context: context,
            }),
            Self::Event { event, fields } => {
                let mut fields = fields.clone();
                fields.remove("hookEventName");
                serde_json::to_value(EventOutputWire {
                    hook_event_name: event.as_str(),
                    fields: &fields,
                })
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SyncHookOutput {
    pub continuation: Option<HookContinuation>,
    pub visibility: Option<HookOutputVisibility>,
    pub stop_reason: Option<String>,
    pub block_reason: Option<String>,
    pub system_message: Option<String>,
    pub reason: Option<String>,
    pub specific: Option<HookSpecificOutput>,
}

#[derive(Clone, Debug)]
pub enum HookOutput {
    Sync(Box<SyncHookOutput>),
    Async {
        timeout: Option<Timeout>,
    },
    /// An upstream `HookJSONOutput`, sent exactly as given.
    Json(Box<HookJSONOutput>),
}

impl From<HookJSONOutput> for HookOutput {
    fn from(output: HookJSONOutput) -> Self {
        Self::Json(Box::new(output))
    }
}

impl Default for HookOutput {
    fn default() -> Self {
        Self::Sync(Box::default())
    }
}

impl HookOutput {
    pub fn block(reason: impl Into<String>) -> Self {
        Self::Sync(Box::new(SyncHookOutput {
            block_reason: Some(reason.into()),
            ..SyncHookOutput::default()
        }))
    }

    pub fn to_wire_json(&self) -> Result<Value, serde_json::Error> {
        match self {
            Self::Json(output) => serde_json::to_value(output),
            Self::Async { timeout } => serde_json::to_value(AsyncHookOutputWire {
                asynchronous: true,
                async_timeout: timeout.map(|timeout| timeout.as_duration().as_millis()),
            }),
            Self::Sync(output) => serde_json::to_value(SyncHookOutputWire {
                continuation: output.continuation.map(|value| value == HookContinuation::Continue),
                suppress_output: output.visibility.map(|value| value == HookOutputVisibility::Suppressed),
                stop_reason: output.stop_reason.as_deref(),
                decision: output.block_reason.as_ref().map(|_| "block"),
                reason: output.reason.as_deref().or(output.block_reason.as_deref()),
                system_message: output.system_message.as_deref(),
                hook_specific_output: output
                    .specific
                    .as_ref()
                    .map(HookSpecificOutput::to_wire_json)
                    .transpose()?,
            }),
        }
    }
}

pub type HookFuture<'a> = Pin<Box<dyn Future<Output = Result<HookOutput, CallbackError>> + Send + 'a>>;

pub trait HookHandler: Send + Sync {
    fn call(&self, request: HookRequest) -> HookFuture<'_>;
}

impl<F, Fut> HookHandler for F
where
    F: Fn(HookRequest) -> Fut + Send + Sync,
    Fut: Future<Output = Result<HookOutput, CallbackError>> + Send + 'static,
{
    fn call(&self, request: HookRequest) -> HookFuture<'_> {
        Box::pin(self(request))
    }
}

#[derive(Clone)]
pub struct HookRegistration {
    pub event: HookEvent,
    pub matcher: Option<String>,
    pub callbacks: Vec<Arc<dyn HookHandler>>,
    pub timeout: Option<Timeout>,
    /// `None` omits `onFailure` from the wire, as both upstream SDKs do when it is unset.
    pub on_failure: Option<HookFailureMode>,
}

impl HookRegistration {
    pub fn to_wire_json(&self, callback_ids: &[String]) -> Result<Value, serde_json::Error> {
        serde_json::to_value(HookRegistrationWire {
            matcher: self.matcher.as_deref(),
            hook_callback_ids: callback_ids,
            on_failure: self.on_failure.map(HookFailureMode::as_str),
            timeout: self.timeout.map(|timeout| timeout.as_duration().as_secs_f64()),
        })
    }
}

impl std::fmt::Debug for HookRegistration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HookRegistration")
            .field("event", &self.event)
            .field("callback_count", &self.callbacks.len())
            .field("timeout", &self.timeout)
            .field("on_failure", &self.on_failure)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[path = "hooks_tests.rs"]
mod tests;
