//! Person Server endpoint bodies per draft section "Person Server": token endpoint,
//! user interaction, clarification chat, permission endpoint, audit endpoint,
//! interaction endpoint, and re-authorization (which defines no new wire shapes).

use serde::{Deserialize, Serialize};

use super::MissionRef;

/// Agent token request body sent to the PS's `token_endpoint`, per "Agent Token
/// Request". All fields but `resource_token` are optional per the draft.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TokenRequest {
    pub resource_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub justification: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
}

impl TokenRequest {
    #[must_use]
    pub fn new(resource_token: impl Into<String>) -> Self {
        Self {
            resource_token: resource_token.into(),
            ..Default::default()
        }
    }
}

/// PS direct grant response (`200`) per "PS Response".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenGrantResponse {
    pub auth_token: String,
    pub expires_in: i64,
}

/// Pending response body (`202`) per "Pending Response". `status` is a string
/// (not an enum) because the draft requires unrecognized values to be treated as
/// `"pending"`, which an exhaustive Rust enum cannot represent without a fallback
/// variant; consumers compare against [`PendingStatus`] helpers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingResponse {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clarification: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_claims: Option<Vec<String>>,
}

impl PendingResponse {
    #[must_use]
    pub fn pending() -> Self {
        Self {
            status: PendingStatus::Pending.as_str().to_string(),
            clarification: None,
            timeout: None,
            options: None,
            required_claims: None,
        }
    }

    #[must_use]
    pub fn is_interacting(&self) -> bool {
        self.status == PendingStatus::Interacting.as_str()
    }
}

/// Well-known `status` values for [`PendingResponse`], per "Pending Response".
/// Agents MUST treat unrecognized values as `"pending"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingStatus {
    Pending,
    Interacting,
}

impl PendingStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            PendingStatus::Pending => "pending",
            PendingStatus::Interacting => "interacting",
        }
    }
}

/// Resource-initiated interaction claim carried in a resource token, per
/// "Resource Token Structure" (`interaction` optional payload claim) and used
/// by "Resource-Initiated Interaction".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceInteraction {
    pub url: String,
    pub code: String,
}

/// Clarification required body fields on a `202` per "Clarification Required".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarificationRequired {
    pub status: String,
    pub clarification: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
}

/// Agent's response to a clarification: `action: clarification_response`, per
/// "Clarification Response".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClarificationResponseRequest {
    pub action: ClarificationAction,
    pub clarification_response: String,
}

/// Agent's updated request in response to a clarification: `action:
/// updated_request`, per "Updated Request".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdatedRequest {
    pub action: ClarificationAction,
    pub resource_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub justification: Option<String>,
}

/// The `action` discriminator used on POSTs to a pending clarification URL, per
/// "Agent Response to Clarification".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClarificationAction {
    ClarificationResponse,
    UpdatedRequest,
}

/// Permission request body sent to the PS's `permission_endpoint`, per
/// "Permission Request".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission: Option<MissionRef>,
}

/// Permission response body (`200`) per "Permission Response".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionResponse {
    pub permission: PermissionDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `permission` field values per "Permission Response".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Granted,
    Denied,
}

/// Audit request body sent to the PS's `audit_endpoint`, per "Audit Request".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRequest {
    pub mission: MissionRef,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}

/// Interaction request body sent to the PS's `interaction_endpoint`, per
/// "Interaction Request".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractionRequest {
    #[serde(rename = "type")]
    pub type_: InteractionRequestType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wait: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission: Option<MissionRef>,
}

/// `type` values for [`InteractionRequest`], per "Interaction Request".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionRequestType {
    Interaction,
    Payment,
    Question,
    Completion,
}

/// Terminal response to an interaction request of `type: question`, per
/// "Interaction Response".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnswer {
    pub answer: String,
}

#[cfg(test)]
mod tests;
