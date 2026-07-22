use trogon_identity_types::aauth::{AgentClaims, Cnf, ResourceClaims};

use super::*;

#[test]
fn as_token_context_carries_all_verified_inputs() {
    let agent = AgentClaims {
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
    };
    let resource = ResourceClaims {
        iss: "https://resource.example".into(),
        aud: "https://as.example".into(),
        jti: "resource-jti".into(),
        iat: 0,
        exp: 1000,
        dwk: "aauth-resource.json".into(),
        agent: agent.sub.clone(),
        agent_jkt: "jkt-1".into(),
        scope: "documents:read".into(),
        mission: None,
    };
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
    assert_eq!(ctx.resource_claims.scope, "documents:read");
    assert_eq!(ctx.agent_claims.sub, "aauth:asst@agent.example");
    assert!(ctx.subagent_claims.is_none());
    assert!(ctx.upstream.is_none());
    assert_eq!(ctx.binding.agent, "aauth:asst@agent.example");
}

#[test]
fn binding_agent_equality() {
    let a = BindingAgent {
        agent: "aauth:asst@agent.example".into(),
        agent_jkt: "jkt-1".into(),
    };
    let b = a.clone();
    assert_eq!(a, b);
}
