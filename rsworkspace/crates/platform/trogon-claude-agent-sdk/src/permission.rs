use std::{future::Future, pin::Pin};

use crate::{
    CancellationToken, ControlRequestId, FeatureSupport, JsonObject, PermissionUpdate, ToolName, ToolUseId,
    upstream::tool_inputs::ToolInputSchemas,
};

#[derive(Clone, Debug)]
pub struct ToolPermissionRequest {
    pub tool_name: ToolName,
    pub input: JsonObject,
    pub tool_use_id: Option<ToolUseId>,
    pub blocked_path: Option<String>,
    pub decision_reason: Option<String>,
    pub request_id: ControlRequestId,
    pub signal: CancellationToken,
    pub suggestions: Vec<PermissionUpdate>,
    pub agent_id: Option<String>,
    pub mcp_server: Option<McpServerProvenance>,
    pub title: Option<String>,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub default_to_no: FeatureSupport,
    pub suppress_always_allow_rule: FeatureSupport,
    pub requires_user_interaction: FeatureSupport,
    /// TypeScript passes `decisionReasonType` only when the wire carries a string.
    pub decision_reason_type: Option<String>,
    /// TypeScript passes `decisionReasonCode` only when the wire carries a string.
    pub decision_reason_code: Option<String>,
    /// TypeScript passes `serverPolicyAskOnly` only when the wire carries a boolean.
    pub server_policy_ask_only: FeatureSupport,
    /// TypeScript passes `plugin` only when the wire carries a string.
    pub plugin: Option<String>,
    /// TypeScript passes `serverPrompt` only when the wire carries a string.
    pub server_prompt: Option<String>,
    /// TypeScript passes `computerFolder` only when the wire carries an object with string `path` and
    /// `computer_name`.
    pub computer_folder: Option<ComputerFolder>,
    pub matched_ask_rule: Option<MatchedAskRule>,
    /// Wire keys the handler context does not name, and named keys whose value had a type the upstream SDK
    /// does not pass on, untouched and in wire order.
    pub additional_fields: JsonObject,
}

impl ToolPermissionRequest {
    /// `input` as the `sdk-tools` input type of the tool `tool_name` names (see [`ToolInputSchemas::for_tool`]).
    /// `input` stays as written.
    pub fn typed_input(&self) -> ToolInputSchemas {
        ToolInputSchemas::for_tool(self.tool_name.as_str(), &serde_json::Value::Object(self.input.clone()))
    }
}

#[derive(Clone, Debug)]
pub struct McpServerProvenance {
    pub name: String,
    pub source: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComputerFolder {
    pub path: String,
    pub computer_name: String,
}

/// The `permissions.ask` rule that forced the prompt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MatchedAskRule {
    pub source: String,
    pub tool_name: String,
    pub rule_content: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DenialDisposition {
    #[default]
    Continue,
    Interrupt,
}

/// TypeScript `PermissionDecisionClassification`: what the user actually chose, for the CLI's telemetry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionDecisionClassification {
    UserTemporary,
    UserPermanent,
    UserReject,
}

impl PermissionDecisionClassification {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserTemporary => "user_temporary",
            Self::UserPermanent => "user_permanent",
            Self::UserReject => "user_reject",
        }
    }
}

#[derive(Clone, Debug)]
pub enum PermissionDecision {
    /// TypeScript sends `updatedInput` only when it is set; Python sends the original input when it is not.
    Allow {
        updated_input: Option<JsonObject>,
        updated_permissions: Option<Vec<PermissionUpdate>>,
        decision_classification: Option<PermissionDecisionClassification>,
    },
    /// [`DenialDisposition::Continue`] leaves `interrupt` out, as both SDKs do for an unset or false value.
    Deny {
        message: String,
        disposition: DenialDisposition,
        decision_classification: Option<PermissionDecisionClassification>,
    },
    /// TypeScript `canUseTool` resolving to `null`: the host answered out of band, so no response is written.
    /// Python rejects the value with its `TypeError` response.
    AnsweredOutOfBand,
    /// The handler failed: both SDKs answer with an error response carrying this message.
    Failed { message: String },
}

impl PermissionDecision {
    pub fn allow() -> Self {
        Self::Allow {
            updated_input: None,
            updated_permissions: None,
            decision_classification: None,
        }
    }

    /// Allows the call with `input` as `updatedInput`, encoded exactly as the typed value encodes. `None` when
    /// `input` does not encode to a JSON object, which is all `updatedInput` carries.
    pub fn allow_with_input(input: &ToolInputSchemas) -> Option<Self> {
        let serde_json::Value::Object(updated_input) = serde_json::to_value(input).ok()? else {
            return None;
        };
        Some(Self::Allow {
            updated_input: Some(updated_input),
            updated_permissions: None,
            decision_classification: None,
        })
    }

    pub fn deny(message: impl Into<String>) -> Self {
        Self::Deny {
            message: message.into(),
            disposition: DenialDisposition::Continue,
            decision_classification: None,
        }
    }
}

pub type PermissionFuture<'a> = Pin<Box<dyn Future<Output = PermissionDecision> + Send + 'a>>;

pub trait ToolPermissionHandler: Send + Sync {
    fn decide(&self, request: ToolPermissionRequest) -> PermissionFuture<'_>;
}

impl<F, Fut> ToolPermissionHandler for F
where
    F: Fn(ToolPermissionRequest) -> Fut + Send + Sync,
    Fut: Future<Output = PermissionDecision> + Send + 'static,
{
    fn decide(&self, request: ToolPermissionRequest) -> PermissionFuture<'_> {
        Box::pin(self(request))
    }
}

impl std::fmt::Debug for dyn ToolPermissionHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ToolPermissionHandler").finish_non_exhaustive()
    }
}
