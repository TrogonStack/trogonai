use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(String);

#[derive(Debug)]
pub struct RequestIdError(&'static str);

impl fmt::Display for RequestIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for RequestIdError {}

impl RequestId {
    pub fn new(raw: impl Into<String>) -> Result<Self, RequestIdError> {
        let value = raw.into();
        if value.trim().is_empty() {
            return Err(RequestIdError("request_id must be non-empty"));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ApprovalSubject(String);

impl ApprovalSubject {
    pub fn for_request(id: &RequestId) -> Self {
        Self(format!("mcp.approvals.{}", id.as_str()))
    }

    pub fn for_step_up(id: &RequestId) -> Self {
        Self(format!("mcp.approvals.step-up.{}", id.as_str()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ArgsHash([u8; 32]);

impl ArgsHash {
    pub fn from_json(args: &serde_json::Value) -> Self {
        let canonical = serde_json::to_vec(args).unwrap_or_default();
        Self(Sha256::digest(canonical).into())
    }

    #[must_use]
    pub fn hex(&self) -> String {
        hex::encode(self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalDecisionKind {
    Approve,
    Deny,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ApprovalDecisionMessage {
    pub decision: ApprovalDecisionKind,
    pub approver: String,
    pub expires_at: i64,
}

#[derive(Clone, Debug)]
pub enum ApprovalWaitOutcome {
    Approved {
        approver: String,
        expires_at: i64,
    },
    Denied {
        reason: String,
    },
    TimedOut,
}

#[derive(Clone, Debug)]
pub enum ApprovalError {
    InvalidDecision(String),
    Subscribe(String),
    Publish(String),
}

impl fmt::Display for ApprovalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDecision(msg) | Self::Subscribe(msg) | Self::Publish(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for ApprovalError {}
