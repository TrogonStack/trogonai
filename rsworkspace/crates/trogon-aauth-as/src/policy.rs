//! Organization policy decision point per draft "AS Decision Logic
//! (Non-Normative)" (#as-decision-logic). The draft explicitly leaves the
//! decision order and criteria to the AS's discretion ("The AS is not
//! required to follow this order"), so this module defines the decision
//! *shape* -- issue, deny, or require more before it can decide -- as a
//! trait, and leaves the ordering/criteria pluggable per deployment.

use trogon_identity_types::aauth::federation::ClaimsSubmission;

use crate::request::AsTokenContext;
use crate::trust::TrustedIssuer;

/// Scope the AS is willing to grant, per "Auth Token Structure" (`scope`
/// conditional claim) and "AS Decision Logic" item 4 ("Grant with restricted
/// scope or rate limits"). A newtype instead of a bare `String` so callers
/// cannot confuse a raw resource-token scope with a policy-narrowed grant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantedScope(String);

impl GrantedScope {
    #[must_use]
    pub fn new(scope: impl Into<String>) -> Self {
        Self(scope.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Claim names the AS needs before it can decide, per "Claims Required"
/// (#requirement-claims): `required_claims` array in the `202` body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequiredClaims(Vec<String>);

impl RequiredClaims {
    pub fn new(claims: impl IntoIterator<Item = impl Into<String>>) -> Result<Self, RequiredClaimsError> {
        let names: Vec<String> = claims.into_iter().map(Into::into).collect();
        if names.is_empty() {
            return Err(RequiredClaimsError::Empty);
        }
        Ok(Self(names))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RequiredClaimsError {
    #[error("required_claims must not be empty when requirement=claims")]
    Empty,
}

/// Outcome of an organization policy evaluation for one PS-to-AS token
/// request, per "AS Response" and "AS Decision Logic (Non-Normative)".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Grant per "AS Response" direct grant (`200`).
    Issue { scope: GrantedScope },
    /// `202 Accepted` / `requirement=claims` (#requirement-claims): the AS
    /// needs identity claims before it can decide.
    RequireClaims { required: RequiredClaims },
    /// Deny per "AS Decision Logic" item 8 ("Insufficient trust for
    /// requested scope") -- surfaced as a token endpoint error.
    Deny { reason: String },
}

/// Organization resource policy, evaluated per "Access Server" in "Policy
/// Evaluation Points" (#policy-evaluation-points): "decides whether to issue
/// an auth token on behalf of the resource -- based on resource policy, the
/// claims the PS has provided, and any further requirements ... gathered via
/// deferred responses."
///
/// Implementations are synchronous by design: policy evaluation in this
/// crate is a pure function of the request context plus any claims already
/// gathered. Deployments that need to call out to an external PDP should do
/// so before constructing the [`AsTokenContext`] passed in, keeping the
/// core AS orchestration (`AccessServer`) free of I/O concerns.
pub trait OrganizationPolicy: Send + Sync {
    /// First evaluation of a PS-to-AS token request, before any claims round
    /// trip. `trust` is the registry entry for the requesting PS (already
    /// verified trusted by the caller).
    fn decide(&self, ctx: &AsTokenContext<'_>, trust: &TrustedIssuer) -> Decision;

    /// Resume evaluation after the PS has POSTed a [`ClaimsSubmission`] in
    /// response to a prior `RequireClaims` decision, per "Claims Required":
    /// "Claims not recognized by the recipient SHOULD be ignored." A policy
    /// MAY require another round of claims (e.g. claims were insufficient) by
    /// returning `RequireClaims` again.
    fn decide_with_claims(
        &self,
        ctx: &AsTokenContext<'_>,
        trust: &TrustedIssuer,
        claims: &ClaimsSubmission,
    ) -> Decision;
}

/// A policy that always issues the resource token's own scope unchanged.
/// Useful as a default for tests and for organizations with no
/// scope-narrowing policy of their own.
#[derive(Clone, Copy, Debug, Default)]
pub struct AlwaysIssuePolicy;

impl OrganizationPolicy for AlwaysIssuePolicy {
    fn decide(&self, ctx: &AsTokenContext<'_>, _trust: &TrustedIssuer) -> Decision {
        Decision::Issue {
            scope: GrantedScope::new(ctx.resource_claims.scope.clone()),
        }
    }

    fn decide_with_claims(
        &self,
        ctx: &AsTokenContext<'_>,
        trust: &TrustedIssuer,
        _claims: &ClaimsSubmission,
    ) -> Decision {
        self.decide(ctx, trust)
    }
}

#[cfg(test)]
mod tests;
