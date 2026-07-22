use trogon_identity_types::aauth::{AgentClaims, Cnf, ResourceClaims};

use super::*;
use crate::request::BindingAgent;
use crate::trust::{PsIssuer, TrustBasisRecord, TrustedIssuer};
use crate::verify::VerifiedRequest;

fn verified_request() -> VerifiedRequest {
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
    VerifiedRequest {
        resource_claims: resource,
        agent_claims: agent,
        subagent_claims: None,
        upstream: None,
        binding,
    }
}

#[test]
fn store_insert_then_take_returns_entry_once() {
    let store = PendingRequestStore::new();
    let id = PendingRequestId::new("pending-1");
    let trust = TrustedIssuer::new(
        PsIssuer::new("https://ps.example").unwrap(),
        TrustBasisRecord::PreEstablished,
    );
    store.insert(
        id.clone(),
        PendingRequest {
            verified: verified_request(),
            trust,
        },
    );

    let taken = store.take(&id);
    assert!(taken.is_some());
    assert_eq!(taken.unwrap().verified.resource_claims.scope, "documents:read");

    assert!(store.take(&id).is_none());
}

#[test]
fn store_take_missing_id_returns_none() {
    let store = PendingRequestStore::new();
    let id = PendingRequestId::new("does-not-exist");
    assert!(store.take(&id).is_none());
}

#[test]
fn pending_request_id_round_trips_as_str() {
    let id = PendingRequestId::new("abc-123");
    assert_eq!(id.as_str(), "abc-123");
}
