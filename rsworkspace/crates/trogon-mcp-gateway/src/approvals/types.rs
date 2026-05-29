use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::approvals::errors::ApprovalError;

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

#[derive(Clone, Debug)]
pub struct ApprovalRequest {
    pub request_id: RequestId,
    pub ttl: Duration,
    pub approval_subject: ApprovalSubject,
    pub approval_url: Option<String>,
}

impl ApprovalRequest {
    pub fn new(
        request_id: RequestId,
        ttl: Duration,
        approval_url: Option<String>,
    ) -> Self {
        let approval_subject = ApprovalSubject::for_request(&request_id);
        Self {
            request_id,
            ttl,
            approval_subject,
            approval_url,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApprovalDecision {
    Granted {
        approver: String,
        expires_at: i64,
    },
    Denied {
        approver: String,
        reason: String,
    },
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

pub fn wire_to_decision(message: &ApprovalDecisionMessage) -> Result<ApprovalDecision, ApprovalError> {
    let now = now_unix();
    if message.expires_at <= now {
        return Err(ApprovalError::MalformedDecision);
    }
    match message.decision {
        ApprovalDecisionKind::Approve => Ok(ApprovalDecision::Granted {
            approver: message.approver.clone(),
            expires_at: message.expires_at,
        }),
        ApprovalDecisionKind::Deny => Ok(ApprovalDecision::Denied {
            approver: message.approver.clone(),
            reason: format!("denied by {}", message.approver),
        }),
    }
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
pub enum ApprovalClientError {
    InvalidDecision(String),
    Subscribe(String),
    Publish(String),
}

impl fmt::Display for ApprovalClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDecision(msg) | Self::Subscribe(msg) | Self::Publish(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for ApprovalClientError {}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_to_decision_approve() {
        let message = ApprovalDecisionMessage {
            decision: ApprovalDecisionKind::Approve,
            approver: "human:alice".into(),
            expires_at: now_unix() + 300,
        };
        assert!(matches!(
            wire_to_decision(&message),
            Ok(ApprovalDecision::Granted { .. })
        ));
    }

    #[test]
    fn wire_to_decision_rejects_expired() {
        let message = ApprovalDecisionMessage {
            decision: ApprovalDecisionKind::Approve,
            approver: "human:alice".into(),
            expires_at: now_unix() - 1,
        };
        assert_eq!(wire_to_decision(&message), Err(ApprovalError::MalformedDecision));
    }
}

