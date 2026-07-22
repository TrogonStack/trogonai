//! Pluggable person-policy decision, per "PS Response": the PS "applies the
//! person's policy" to a verified token request and returns one of a grant,
//! a denial, a request for interaction, a request for clarification, or a
//! pending approval from a third party (e.g. another device, a delegate).
//!
//! This crate does not ship an opinionated policy engine -- "the person's
//! policy" is deployment-specific (allow-lists, ML classifiers, human
//! review, mission scope checks, etc). [`PolicyEngine`] is the seam a host
//! application implements.

use async_trait::async_trait;
use trogon_identity_types::aauth::{AgentClaims, ResourceClaims};

use crate::mission::MissionContext;

/// Everything the policy needs to decide one token request, already verified
/// by [`crate::PersonServer`] before policy runs.
pub struct DecisionRequest<'a> {
    pub agent: &'a AgentClaims,
    pub resource: &'a ResourceClaims,
    pub justification: Option<&'a str>,
    /// Present when the resource token references an approved mission, per
    /// "Mission" (#mission): mission context can widen or narrow what the
    /// policy grants relative to a mission-less request.
    pub mission: Option<&'a MissionContext>,
    /// `0` on the first evaluation of a pending request; incremented each
    /// time the agent responds to a clarification, per "Clarification
    /// Limits".
    pub clarification_round: u32,
}

/// A person-policy decision, per "PS Response" and "Clarification Chat".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyDecision {
    /// Grant the requested scope, per "PS Response" `200`. May narrow the
    /// resource token's requested scope (e.g. read-only instead of read-write).
    Grant { scope: String },
    /// Deny outright. Distinct from [`PolicyDecision::NeedsInteraction`] /
    /// [`PolicyDecision::NeedsClarification`]: denial is terminal for this
    /// request, per "Token Endpoint Error Codes" `invalid_request`-class
    /// handling at the caller.
    Deny { reason: String },
    /// Requires the user to interact out-of-band (approve/deny via a link or
    /// device), per "User Interaction" (#user-interaction).
    NeedsInteraction,
    /// Requires a clarification round-trip with the agent before a decision
    /// can be made, per "Clarification Chat" (#clarification-chat).
    NeedsClarification {
        clarification: String,
        options: Option<Vec<String>>,
    },
    /// Approval is pending from a third party the PS itself does not
    /// control, per "Requirement Values" `requirement=approval`.
    ApprovalPending,
}

/// Deployment-specific policy engine. Implementations decide grant/deny and
/// may consult mission context, external services, or a human reviewer.
#[async_trait]
pub trait PolicyEngine: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn decide(&self, request: DecisionRequest<'_>) -> Result<PolicyDecision, Self::Error>;
}

#[cfg(test)]
mod tests;
