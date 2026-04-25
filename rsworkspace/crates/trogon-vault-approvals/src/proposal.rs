//! Proposal data model and message types.

use serde::{Deserialize, Serialize};

// ── Domain types ──────────────────────────────────────────────────────────────

/// A pending or resolved approval proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    /// Unique identifier supplied by the requesting agent.
    pub id:             String,
    /// The proxy token (`tok_...`) for which the credential is requested.
    pub credential_key: String,
    /// Upstream service that will receive the credential (e.g. `api.stripe.com`).
    pub service:        String,
    /// Human-readable reason provided by the agent.
    pub message:        String,
    /// Current lifecycle state.
    pub status:         ProposalStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProposalStatus {
    Pending,
    Approved { approved_by: String },
    Rejected { rejected_by: String, reason: String },
}

// ── Request / response message types ─────────────────────────────────────────

/// Published by the agent to `vault.proposals.{vault}.create`.
#[derive(Debug, Deserialize)]
pub struct CreateRequest {
    /// Caller-chosen unique ID — `prop_{random}` by convention.
    pub id:             String,
    pub credential_key: String,
    pub service:        String,
    pub message:        String,
}

/// Published by a human approver to `vault.proposals.{vault}.approve`.
#[derive(Debug, Deserialize)]
pub struct ApproveRequest {
    pub proposal_id: String,
    pub approved_by: String,
    /// The plaintext API key — never stored in JetStream, consumed immediately.
    pub plaintext:   String,
}

/// Published by a human approver to `vault.proposals.{vault}.reject`.
#[derive(Debug, Deserialize)]
pub struct RejectRequest {
    pub proposal_id: String,
    pub rejected_by: String,
    #[serde(default)]
    pub reason:      String,
}

/// Reply to `vault.proposals.{vault}.status.{id}`.
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub proposal_id: String,
    /// One of: `"pending"`, `"approved"`, `"rejected"`, `"not_found"`.
    pub status:      String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejected_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason:      Option<String>,
}

impl StatusResponse {
    pub fn not_found(proposal_id: String) -> Self {
        Self { proposal_id, status: "not_found".into(), approved_by: None, rejected_by: None, reason: None }
    }

    pub fn from_proposal(p: &Proposal) -> Self {
        match &p.status {
            ProposalStatus::Pending => Self {
                proposal_id: p.id.clone(),
                status:      "pending".into(),
                approved_by: None,
                rejected_by: None,
                reason:      None,
            },
            ProposalStatus::Approved { approved_by } => Self {
                proposal_id: p.id.clone(),
                status:      "approved".into(),
                approved_by: Some(approved_by.clone()),
                rejected_by: None,
                reason:      None,
            },
            ProposalStatus::Rejected { rejected_by, reason } => Self {
                proposal_id: p.id.clone(),
                status:      "rejected".into(),
                approved_by: None,
                rejected_by: Some(rejected_by.clone()),
                reason:      Some(reason.clone()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_request_round_trips() {
        let json = r#"{"id":"prop_abc","credential_key":"tok_stripe_prod_abc1","service":"api.stripe.com","message":"need stripe"}"#;
        let req: CreateRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.id, "prop_abc");
    }

    #[test]
    fn approve_request_round_trips() {
        let json = r#"{"proposal_id":"prop_abc","approved_by":"mario","plaintext":"sk_live_xyz"}"#;
        let req: ApproveRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.plaintext, "sk_live_xyz");
    }

    #[test]
    fn reject_request_defaults_empty_reason() {
        let json = r#"{"proposal_id":"prop_abc","rejected_by":"mario"}"#;
        let req: RejectRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.reason, "");
    }

    #[test]
    fn status_response_not_found() {
        let resp = StatusResponse::not_found("prop_x".into());
        let v: serde_json::Value = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["status"], "not_found");
        assert!(v.get("approved_by").is_none());
    }

    #[test]
    fn status_response_from_approved_proposal() {
        let proposal = Proposal {
            id:             "prop_abc".into(),
            credential_key: "tok_stripe_prod_abc1".into(),
            service:        "api.stripe.com".into(),
            message:        "need stripe".into(),
            status:         ProposalStatus::Approved { approved_by: "mario".into() },
        };
        let resp = StatusResponse::from_proposal(&proposal);
        assert_eq!(resp.status, "approved");
        assert_eq!(resp.approved_by.as_deref(), Some("mario"));
    }
}
