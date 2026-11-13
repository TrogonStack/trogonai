use std::{future::Future, pin::Pin};

use crate::{CancellationToken, ControlRequestId, FeatureSupport, JsonObject, PermissionUpdate, ToolName, ToolUseId};

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
    pub additional_fields: JsonObject,
}

#[derive(Clone, Debug)]
pub struct McpServerProvenance {
    pub name: String,
    pub source: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DenialDisposition {
    #[default]
    Continue,
    Interrupt,
}

#[derive(Clone, Debug)]
pub enum PermissionDecision {
    Allow {
        updated_input: Option<JsonObject>,
        updated_permissions: Option<Vec<PermissionUpdate>>,
    },
    Deny {
        message: String,
        disposition: DenialDisposition,
    },
}

impl PermissionDecision {
    pub fn allow() -> Self {
        Self::Allow {
            updated_input: None,
            updated_permissions: None,
        }
    }

    pub fn deny(message: impl Into<String>) -> Self {
        Self::Deny {
            message: message.into(),
            disposition: DenialDisposition::Continue,
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
