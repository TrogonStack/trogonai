//! Verified inputs to one PS-to-AS token request, per "PS-to-AS Token
//! Request" (#as-token-endpoint). [`crate::server::AccessServer`] builds this
//! from the raw [`trogon_identity_types::aauth::federation::AsTokenRequest`]
//! after verifying the agent token, resource token binding, and any upstream
//! token, so [`crate::policy::OrganizationPolicy`] only ever sees
//! already-verified, typed data.

use trogon_aauth_verify::VerifiedAuth;
use trogon_identity_types::aauth::{AgentClaims, ResourceClaims};

/// Which agent the issued auth token binds to, per "PS-to-AS Token Request":
/// ordinarily the requesting `agent_token`'s subject, or -- for
/// parent-mediated sub-agent authorization -- the `subagent_token`'s
/// subject, with the parent recorded in `act` (#sub-agents).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingAgent {
    pub agent: String,
    pub agent_jkt: String,
}

/// Verified, typed view of one AS token request, assembled by
/// [`crate::server::AccessServer::evaluate`] before handing off to policy.
pub struct AsTokenContext<'a> {
    /// The resource token the resource issued (`aud` = this AS), verified.
    pub resource_claims: &'a ResourceClaims,
    /// The verified requesting agent token's claims (the parent, in the
    /// sub-agent case; see "PS-to-AS Token Request").
    pub agent_claims: &'a AgentClaims,
    /// The verified sub-agent token's claims, present only for
    /// parent-mediated sub-agent authorization (#sub-agents).
    pub subagent_claims: Option<&'a AgentClaims>,
    /// The verified upstream auth token, present only in call chaining
    /// (#call-chaining), already checked per "Upstream Token Verification".
    pub upstream: Option<&'a VerifiedAuth>,
    /// The agent identifier and key the issued auth token will bind to.
    pub binding: &'a BindingAgent,
}

#[cfg(test)]
mod tests;
