use trogon_identity_types::aauth::federation::ClaimsSubmission;
use trogon_identity_types::aauth::{AgentClaims, Cnf, ResourceClaims};

use super::*;
use crate::request::BindingAgent;
use crate::trust::{PsIssuer, TrustBasisRecord, TrustedIssuer};

fn agent_claims() -> AgentClaims {
    AgentClaims {
        iss: "https://ap.example".into(),
        sub: "aauth:asst@agent.example".into(),
        jti: "agent-jti".into(),
        iat: 0,
        exp: 1000,
        dwk: "aauth-agent.json".into(),
        cnf: Cnf {
            jwk: serde_json::json!({"kty": "EC"}),
        },
        ps: None,
    }
}

fn resource_claims(scope: &str) -> ResourceClaims {
    ResourceClaims {
        iss: "https://resource.example".into(),
        aud: "https://as.example".into(),
        jti: "resource-jti".into(),
        iat: 0,
        exp: 1000,
        dwk: "aauth-resource.json".into(),
        agent: "aauth:asst@agent.example".into(),
        agent_jkt: "jkt-1".into(),
        scope: scope.into(),
        mission: None,
    }
}

fn trusted_issuer() -> TrustedIssuer {
    TrustedIssuer::new(
        PsIssuer::new("https://ps.example").unwrap(),
        TrustBasisRecord::PreEstablished,
    )
}

#[test]
fn granted_scope_round_trips() {
    let scope = GrantedScope::new("read write");
    assert_eq!(scope.as_str(), "read write");
}

#[test]
fn required_claims_rejects_empty() {
    let err = RequiredClaims::new(Vec::<String>::new()).unwrap_err();
    assert_eq!(err, RequiredClaimsError::Empty);
}

#[test]
fn required_claims_accepts_names() {
    let claims = RequiredClaims::new(["email", "tenant"]).unwrap();
    assert_eq!(claims.as_slice(), &["email".to_string(), "tenant".to_string()]);
}

#[test]
fn always_issue_policy_issues_resource_token_scope() {
    let policy = AlwaysIssuePolicy;
    let resource = resource_claims("documents:read");
    let agent = agent_claims();
    let binding = BindingAgent {
        agent: agent.sub.clone(),
        agent_jkt: "jkt-1".into(),
    };
    let ctx = AsTokenContext {
        resource_claims: &resource,
        agent_claims: &agent,
        subagent_claims: None,
        upstream: None,
        binding: &binding,
    };
    let trust = trusted_issuer();
    let decision = policy.decide(&ctx, &trust);
    assert_eq!(
        decision,
        Decision::Issue {
            scope: GrantedScope::new("documents:read")
        }
    );
}

#[test]
fn always_issue_policy_decide_with_claims_matches_decide() {
    let policy = AlwaysIssuePolicy;
    let resource = resource_claims("documents:read");
    let agent = agent_claims();
    let binding = BindingAgent {
        agent: agent.sub.clone(),
        agent_jkt: "jkt-1".into(),
    };
    let ctx = AsTokenContext {
        resource_claims: &resource,
        agent_claims: &agent,
        subagent_claims: None,
        upstream: None,
        binding: &binding,
    };
    let trust = trusted_issuer();
    let submission = ClaimsSubmission {
        sub: "user:alice".into(),
        claims: serde_json::Map::new(),
    };
    let decision = policy.decide_with_claims(&ctx, &trust, &submission);
    assert_eq!(
        decision,
        Decision::Issue {
            scope: GrantedScope::new("documents:read")
        }
    );
}

/// A policy exercising all three `Decision` variants, keyed off scope, so
/// state-transition tests in `server/tests.rs` can drive issue / deny /
/// claims-required paths without hand-rolling a policy each time.
pub struct ScopeSwitchedPolicy;

impl OrganizationPolicy for ScopeSwitchedPolicy {
    fn decide(&self, ctx: &AsTokenContext<'_>, _trust: &TrustedIssuer) -> Decision {
        match ctx.resource_claims.scope.as_str() {
            "deny-me" => Decision::Deny {
                reason: DenialReason::new("org policy denies this scope"),
            },
            "claims-please" => Decision::RequireClaims {
                required: RequiredClaims::new(["email", "tenant"]).unwrap(),
            },
            scope => Decision::Issue {
                scope: GrantedScope::new(scope),
            },
        }
    }

    fn decide_with_claims(
        &self,
        ctx: &AsTokenContext<'_>,
        _trust: &TrustedIssuer,
        claims: &ClaimsSubmission,
    ) -> Decision {
        if claims.sub.is_empty() {
            return Decision::Deny {
                reason: DenialReason::new("missing directed sub"),
            };
        }
        Decision::Issue {
            scope: GrantedScope::new(ctx.resource_claims.scope.clone()),
        }
    }
}

#[test]
fn scope_switched_policy_denies() {
    let policy = ScopeSwitchedPolicy;
    let resource = resource_claims("deny-me");
    let agent = agent_claims();
    let binding = BindingAgent {
        agent: agent.sub.clone(),
        agent_jkt: "jkt-1".into(),
    };
    let ctx = AsTokenContext {
        resource_claims: &resource,
        agent_claims: &agent,
        subagent_claims: None,
        upstream: None,
        binding: &binding,
    };
    let decision = policy.decide(&ctx, &trusted_issuer());
    assert!(matches!(decision, Decision::Deny { .. }));
}

#[test]
fn scope_switched_policy_requires_claims_then_issues() {
    let policy = ScopeSwitchedPolicy;
    let resource = resource_claims("claims-please");
    let agent = agent_claims();
    let binding = BindingAgent {
        agent: agent.sub.clone(),
        agent_jkt: "jkt-1".into(),
    };
    let ctx = AsTokenContext {
        resource_claims: &resource,
        agent_claims: &agent,
        subagent_claims: None,
        upstream: None,
        binding: &binding,
    };
    let trust = trusted_issuer();
    let first = policy.decide(&ctx, &trust);
    assert!(matches!(first, Decision::RequireClaims { .. }));

    let submission = ClaimsSubmission {
        sub: "user:alice".into(),
        claims: serde_json::Map::new(),
    };
    let second = policy.decide_with_claims(&ctx, &trust, &submission);
    assert_eq!(
        second,
        Decision::Issue {
            scope: GrantedScope::new("claims-please")
        }
    );
}
