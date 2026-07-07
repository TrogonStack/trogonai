use jsonwebtoken::Algorithm;
use trogon_aauth_verify::{StaticJwks, SystemTimeSource, TokenVerifier};
use trogon_identity_types::aauth::federation::{AsTokenRequest, ClaimsSubmission};

use super::*;
use crate::policy::{AlwaysIssuePolicy, Decision, GrantedScope, OrganizationPolicy, RequiredClaims};
use crate::request::AsTokenContext;
use crate::test_support::{jwk_set, key_fixture, mint_agent_jwt, mint_auth_jwt_raw, mint_resource_jwt, now_unix};
use crate::trust::{PsIssuer, TrustBasisRecord, TrustedIssuer};

const AS_ISS: &str = "https://as.example";
const PS_ISS: &str = "https://ps.example";
const AP_ISS: &str = "https://ap.example";
const RESOURCE_ISS: &str = "https://resource.example";

/// Keys off `resource_claims.scope` so a single policy can drive issue, deny,
/// and claims-required transitions across these tests without a mock crate.
struct ScopeSwitchedPolicy;

impl OrganizationPolicy for ScopeSwitchedPolicy {
    fn decide(&self, ctx: &AsTokenContext<'_>, _trust: &TrustedIssuer) -> Decision {
        match ctx.resource_claims.scope.as_str() {
            "deny-me" => Decision::Deny {
                reason: crate::policy::DenialReason::new("org policy denies this scope"),
            },
            "claims-please" => Decision::RequireClaims {
                required: RequiredClaims::new(["email"]).unwrap(),
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
        _claims: &ClaimsSubmission,
    ) -> Decision {
        Decision::Issue {
            scope: GrantedScope::new(ctx.resource_claims.scope.clone()),
        }
    }
}

fn build_server(
    scope: &str,
    now: i64,
) -> (
    AccessServer<StaticJwks, SystemTimeSource, ScopeSwitchedPolicy>,
    AsTokenRequest,
) {
    let as_key = key_fixture("as-kid");
    let ap_key = key_fixture("ap-kid");
    let resource_key = key_fixture("resource-kid");
    let agent_key = key_fixture("agent-key");

    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid",
        AP_ISS,
        "aauth:asst@agent.example",
        &agent_key.jwk_json,
        None,
        now,
    );
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent_key.jwk_json).unwrap();
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid",
        RESOURCE_ISS,
        AS_ISS,
        "aauth:asst@agent.example",
        &agent_jkt,
        scope,
        now,
    );

    let jwks = StaticJwks::new()
        .with(AP_ISS, jwk_set(&[&ap_key]))
        .with(RESOURCE_ISS, jwk_set(&[&resource_key]))
        .with(AS_ISS, jwk_set(&[&as_key]));
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let trust = TrustRegistry::from_entries([(PS_ISS.to_string(), TrustBasisRecord::PreEstablished)]).unwrap();
    let cfg = AccessServerConfig {
        iss: AS_ISS.to_string(),
        signing_key: as_key.encoding_key,
        alg: Algorithm::ES256,
        kid: "as-kid".to_string(),
        auth_token_ttl: AuthTokenTtl::new(300).unwrap(),
        mode: FederationMode::Federated,
    };

    let server = AccessServer::new(verifier, SystemTimeSource, trust, ScopeSwitchedPolicy, cfg);
    let req = AsTokenRequest {
        resource_token,
        agent_token,
        subagent_token: None,
        upstream_token: None,
    };
    (server, req)
}

#[tokio::test]
async fn evaluate_issues_a_direct_grant() {
    let now = now_unix();
    let (server, req) = build_server("documents:read", now);

    let outcome = server.evaluate(PS_ISS, &req).await.unwrap();
    match outcome {
        AsOutcome::Issued(resp) => assert!(!resp.auth_token.is_empty()),
        _ => panic!("expected Issued"),
    }
}

#[tokio::test]
async fn evaluate_denies_per_policy() {
    let now = now_unix();
    let (server, req) = build_server("deny-me", now);

    let outcome = server.evaluate(PS_ISS, &req).await.unwrap();
    match outcome {
        AsOutcome::Denied { reason } => assert_eq!(reason.as_str(), "org policy denies this scope"),
        _ => panic!("expected Denied"),
    }
}

#[tokio::test]
async fn evaluate_requires_claims_then_resume_issues() {
    let now = now_unix();
    let (server, req) = build_server("claims-please", now);

    let outcome = server.evaluate(PS_ISS, &req).await.unwrap();
    let pending_id = match outcome {
        AsOutcome::ClaimsRequired {
            pending_id,
            required_claims,
        } => {
            assert_eq!(required_claims, vec!["email".to_string()]);
            pending_id
        }
        _ => panic!("expected ClaimsRequired"),
    };

    let submission = ClaimsSubmission {
        sub: "user:alice".into(),
        claims: serde_json::Map::new(),
    };
    let resumed = server
        .resume_with_claims(&pending_id, PS_ISS, &submission)
        .await
        .unwrap();
    match resumed {
        AsOutcome::Issued(resp) => assert!(!resp.auth_token.is_empty()),
        _ => panic!("expected Issued after resume"),
    }
}

#[tokio::test]
async fn resume_with_claims_rejects_unknown_pending_id() {
    let now = now_unix();
    let (server, _req) = build_server("claims-please", now);
    let submission = ClaimsSubmission {
        sub: "user:alice".into(),
        claims: serde_json::Map::new(),
    };
    let err = server
        .resume_with_claims(&PendingRequestId::new("does-not-exist"), PS_ISS, &submission)
        .await
        .unwrap_err();
    assert!(matches!(err, AccessServerError::UnknownPendingRequest(_)));
}

#[tokio::test]
async fn resume_with_claims_rejects_a_different_ps_and_preserves_the_pending_request() {
    let now = now_unix();
    let (server, req) = build_server("claims-please", now);

    let outcome = server.evaluate(PS_ISS, &req).await.unwrap();
    let pending_id = match outcome {
        AsOutcome::ClaimsRequired { pending_id, .. } => pending_id,
        _ => panic!("expected ClaimsRequired"),
    };

    let submission = ClaimsSubmission {
        sub: "user:alice".into(),
        claims: serde_json::Map::new(),
    };
    // A different PS gets the same answer as an unknown id: no probe oracle,
    // no takeover, no burning of the parked request.
    let err = server
        .resume_with_claims(&pending_id, "https://ps-imposter.example", &submission)
        .await
        .unwrap_err();
    assert!(matches!(err, AccessServerError::UnknownPendingRequest(_)));

    // The rightful PS can still resume afterwards.
    let resumed = server
        .resume_with_claims(&pending_id, PS_ISS, &submission)
        .await
        .unwrap();
    assert!(matches!(resumed, AsOutcome::Issued(_)));
}

#[tokio::test]
async fn evaluate_rejects_untrusted_ps() {
    let now = now_unix();
    let (server, req) = build_server("documents:read", now);
    let err = server.evaluate("https://unknown-ps.example", &req).await.unwrap_err();
    assert!(matches!(
        err,
        AccessServerError::Verification(RequestVerificationError::UntrustedPs(_))
    ));
}

#[tokio::test]
async fn collapsed_mode_trusts_own_ps_iss_without_registry_entry() {
    let now = now_unix();
    let as_key = key_fixture("as-kid-c");
    let ap_key = key_fixture("ap-kid-c");
    let resource_key = key_fixture("resource-kid-c");
    let agent_key = key_fixture("agent-key-c");

    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid-c",
        AP_ISS,
        "aauth:asst@agent.example",
        &agent_key.jwk_json,
        None,
        now,
    );
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent_key.jwk_json).unwrap();
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid-c",
        RESOURCE_ISS,
        AS_ISS,
        "aauth:asst@agent.example",
        &agent_jkt,
        "documents:read",
        now,
    );

    let jwks = StaticJwks::new()
        .with(AP_ISS, jwk_set(&[&ap_key]))
        .with(RESOURCE_ISS, jwk_set(&[&resource_key]))
        .with(AS_ISS, jwk_set(&[&as_key]));
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let cfg = AccessServerConfig {
        iss: AS_ISS.to_string(),
        signing_key: as_key.encoding_key,
        alg: Algorithm::ES256,
        kid: "as-kid-c".to_string(),
        auth_token_ttl: AuthTokenTtl::new(300).unwrap(),
        mode: FederationMode::Collapsed {
            ps_iss: PS_ISS.to_string(),
        },
    };
    let server = AccessServer::new(
        verifier,
        SystemTimeSource,
        TrustRegistry::empty(),
        ScopeSwitchedPolicy,
        cfg,
    );
    let req = AsTokenRequest {
        resource_token,
        agent_token,
        subagent_token: None,
        upstream_token: None,
    };

    let outcome = server.evaluate(PS_ISS, &req).await.unwrap();
    assert!(matches!(outcome, AsOutcome::Issued(_)));
}

/// Mandatory coverage: a three-hop call chain (root AS -> mid resource acting
/// as its own agent -> this AS) must produce a minted token whose `act`
/// nests the *entire* chain, per "Upstream Token Verification" step 4 and
/// "Delegation Chain".
#[tokio::test]
async fn act_nesting_preserves_full_chain_across_chained_upstream_tokens() {
    let now = now_unix();
    let as_key = key_fixture("as-kid-n");
    let ap_key = key_fixture("ap-kid-n");
    let resource_key = key_fixture("resource-kid-n");
    let agent_key = key_fixture("agent-key-n");
    let root_as_key = key_fixture("root-as-kid-n");

    // Root grant: root AS issued a token to "aauth:root@agent.example" acting
    // through "aauth:mid@agent.example" (act.agent = mid, no further nesting).
    let root_act = serde_json::json!({ "agent": "aauth:mid@agent.example" });
    let upstream_token = mint_auth_jwt_raw(
        &root_as_key,
        "root-as-kid-n",
        "https://root-as.example",
        AP_ISS,
        "user:alice",
        "aauth:root@agent.example",
        "root-jkt",
        &serde_json::json!({"kty": "EC"}),
        "documents:read",
        Some(root_act.clone()),
        now,
    );

    // This AS's own request: the intermediary resource signs with its own
    // agent_token ("aauth:asst@agent.example") and forwards the upstream token.
    let agent_token = mint_agent_jwt(
        &ap_key,
        "ap-kid-n",
        AP_ISS,
        "aauth:asst@agent.example",
        &agent_key.jwk_json,
        None,
        now,
    );
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent_key.jwk_json).unwrap();
    let resource_token = mint_resource_jwt(
        &resource_key,
        "resource-kid-n",
        RESOURCE_ISS,
        AS_ISS,
        "aauth:asst@agent.example",
        &agent_jkt,
        "documents:read",
        now,
    );

    let jwks = StaticJwks::new()
        .with(AP_ISS, jwk_set(&[&ap_key]))
        .with(RESOURCE_ISS, jwk_set(&[&resource_key]))
        .with(AS_ISS, jwk_set(&[&as_key]))
        .with("https://root-as.example", jwk_set(&[&root_as_key]));
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let mut trust = TrustRegistry::from_entries([(PS_ISS.to_string(), TrustBasisRecord::PreEstablished)]).unwrap();
    trust.trust(
        PsIssuer::new("https://root-as.example").unwrap(),
        TrustBasisRecord::PreEstablished,
    );
    let cfg = AccessServerConfig {
        iss: AS_ISS.to_string(),
        signing_key: as_key.encoding_key,
        alg: Algorithm::ES256,
        kid: "as-kid-n".to_string(),
        auth_token_ttl: AuthTokenTtl::new(300).unwrap(),
        mode: FederationMode::Federated,
    };
    let server = AccessServer::new(verifier, SystemTimeSource, trust, AlwaysIssuePolicy, cfg);
    let req = AsTokenRequest {
        resource_token,
        agent_token,
        subagent_token: None,
        upstream_token: Some(upstream_token),
    };

    let outcome = server.evaluate(PS_ISS, &req).await.unwrap();
    let minted = match outcome {
        AsOutcome::Issued(resp) => resp.auth_token,
        _ => panic!("expected Issued"),
    };

    // Assert the full nested act chain: asst (this AS's own hop) -> mid
    // (root grant's act) -- root's act had no further nesting, so the chain
    // terminates there. Decoded directly off the payload (rather than
    // re-verified) since the signing key was moved into `server` above.
    let claims = decode_claims_unverified(&minted);
    let act = claims.get("act").expect("act present");
    assert_eq!(act["agent"], "aauth:asst@agent.example");
    let nested = act.get("act").expect("nested act present");
    assert_eq!(nested["agent"], "aauth:mid@agent.example");
    assert!(nested.get("act").is_none(), "chain should terminate at root grant");
}

fn decode_claims_unverified(jwt: &str) -> serde_json::Value {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let mut parts = jwt.splitn(3, '.');
    let _header = parts.next().unwrap();
    let payload = parts.next().unwrap();
    let bytes = URL_SAFE_NO_PAD.decode(payload).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
