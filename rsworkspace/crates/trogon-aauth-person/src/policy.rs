//! Consent policy used by the Person Server when exchanging resource tokens.
//!
//! MVP shipping policy: `AllowConfiguredScopes` — for a static map of
//! `(principal, agent_id, resource_iss) → scope`, auto-issue `aa-auth+jwt`. In
//! production this is replaced by an interactive consent flow.

use std::collections::{HashMap, HashSet};

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
    /// Backend (MCP server) the consent is being requested for. Used by
    /// `BackendBypassPolicy` to honor the `consent_disabled` opt-out.
    pub backend: &'a str,
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

/// Wraps another `ConsentPolicy` and skips the consent screen entirely
/// for backends listed in `consent_disabled`. Mirrors Solo.io's
/// `consent_disabled: "true"` elicitation-secret opt-out.
pub struct BackendBypassPolicy<P> {
    inner: P,
    consent_disabled_backends: HashSet<String>,
    bypass_ttl_secs: i64,
}

impl<P> BackendBypassPolicy<P> {
    pub fn new(inner: P, bypass_ttl_secs: i64) -> Self {
        Self {
            inner,
            consent_disabled_backends: HashSet::new(),
            bypass_ttl_secs,
        }
    }

    #[must_use]
    pub fn disable_consent_for(mut self, backend: &str) -> Self {
        self.consent_disabled_backends.insert(backend.into());
        self
    }

    pub fn is_consent_disabled(&self, backend: &str) -> bool {
        self.consent_disabled_backends.contains(backend)
    }
}

#[async_trait]
impl<P: ConsentPolicy> ConsentPolicy for BackendBypassPolicy<P> {
    async fn decide(&self, ctx: &ConsentContext<'_>) -> ConsentDecision {
        if self.is_consent_disabled(ctx.backend) {
            return ConsentDecision::Allow {
                granted_scope: ctx.requested_scope.to_string(),
                ttl_secs: self.bypass_ttl_secs,
            };
        }
        self.inner.decide(ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(backend: &'a str, scope: &'a str) -> ConsentContext<'a> {
        ConsentContext {
            principal: "alice",
            agent_id: "agent",
            resource_iss: "https://mcp.example.com",
            requested_scope: scope,
            backend,
        }
    }

    #[tokio::test]
    async fn backend_bypass_short_circuits_disabled_backend() {
        let inner = AllowConfiguredScopes::new(60);
        let policy = BackendBypassPolicy::new(inner, 300).disable_consent_for("mcp-public");
        let decision = policy.decide(&ctx("mcp-public", "read:tools")).await;
        assert_eq!(
            decision,
            ConsentDecision::Allow {
                granted_scope: "read:tools".into(),
                ttl_secs: 300,
            }
        );
    }

    #[tokio::test]
    async fn backend_bypass_falls_through_for_other_backends() {
        let inner = AllowConfiguredScopes::new(60);
        let policy = BackendBypassPolicy::new(inner, 300).disable_consent_for("mcp-public");
        let decision = policy.decide(&ctx("mcp-private", "read:tools")).await;
        assert!(matches!(decision, ConsentDecision::Deny { .. }));
    }
}
