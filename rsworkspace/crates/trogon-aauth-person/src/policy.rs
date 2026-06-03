//! Consent policy used by the Person Server when exchanging resource tokens.
//!
//! MVP shipping policy: `AllowConfiguredScopes` — for a static map of
//! `(principal, agent_id, resource_iss) → scope`, auto-issue `aa-auth+jwt`. In
//! production this is replaced by an interactive consent flow.

use std::collections::HashMap;

use async_trait::async_trait;

#[async_trait]
pub trait ConsentPolicy: Send + Sync {
    async fn decide(&self, ctx: &ConsentContext<'_>) -> ConsentDecision;
}

#[derive(Clone, Debug)]
pub struct ConsentContext<'a> {
    pub principal: &'a str,
    pub agent_id: &'a str,
    pub resource_iss: &'a str,
    pub requested_scope: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsentDecision {
    Allow { granted_scope: String, ttl_secs: i64 },
    Interaction { url: String, code: Option<String> },
    Deny { reason: String },
}

/// MVP policy: a static allow-list keyed by `(principal, agent_id, resource_iss)`.
/// Unknown tuples are denied.
#[derive(Default, Clone)]
pub struct AllowConfiguredScopes {
    allowed: HashMap<(String, String, String), String>,
    default_ttl_secs: i64,
}

impl AllowConfiguredScopes {
    #[must_use]
    pub fn new(default_ttl_secs: i64) -> Self {
        Self {
            allowed: HashMap::new(),
            default_ttl_secs,
        }
    }

    #[must_use]
    pub fn with(mut self, principal: &str, agent_id: &str, resource_iss: &str, scope: &str) -> Self {
        self.allowed
            .insert((principal.into(), agent_id.into(), resource_iss.into()), scope.into());
        self
    }
}

#[async_trait]
impl ConsentPolicy for AllowConfiguredScopes {
    async fn decide(&self, ctx: &ConsentContext<'_>) -> ConsentDecision {
        match self.allowed.get(&(
            ctx.principal.to_string(),
            ctx.agent_id.to_string(),
            ctx.resource_iss.to_string(),
        )) {
            Some(scope) => ConsentDecision::Allow {
                granted_scope: scope.clone(),
                ttl_secs: self.default_ttl_secs,
            },
            None => ConsentDecision::Deny {
                reason: "no consent on file".into(),
            },
        }
    }
}
