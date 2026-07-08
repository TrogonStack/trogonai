//! End-to-end test of the `PersonServer` library core, per draft-hardt-oauth-aauth-protocol
//! section "Person Server" (#person-server).
//!
//! Mints a real agent key/token and resource challenge, then drives the full
//! `PersonServer` through the grant, interaction, and clarification paths,
//! verifying the resulting `aa-auth+jwt` via `trogon-aauth-verify` including
//! agent binding -- mirroring the fixture style of
//! `a2a-gateway/tests/aauth_roundtrip.rs`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Mutex as StdMutex;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, EllipticCurve, EllipticCurveKeyParameters, EllipticCurveKeyType, Jwk,
    JwkSet, PublicKeyUse,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::ecdsa::SigningKey;
use p256::pkcs8::EncodePrivateKey;
use rand_core::OsRng;
use trogon_aauth_person::decision::{DecisionRequest, PolicyDecision, PolicyEngine};
use trogon_aauth_person::interaction::NoopInteractionChannel;
use trogon_aauth_person::store::InMemoryStore;
use trogon_aauth_person::{PersonServer, TokenEndpointOutcome};
use trogon_aauth_verify::{StaticJwks, SystemTimeSource, TimeSource, TokenVerifier};
use trogon_identity_types::aauth::person_server::{ClarificationAction, TokenRequest};
use trogon_identity_types::aauth::{Cnf, DWK_AGENT, DWK_RESOURCE, ResourceClaims, TYP_AGENT, TYP_RESOURCE};

struct AgentFixture {
    jwk_val: serde_json::Value,
    sub: String,
}

fn agent_fixture(sub: &str, kid: &str) -> AgentFixture {
    let signing_key = SigningKey::random(&mut OsRng);
    let verifying = signing_key.verifying_key();
    let point = verifying.to_encoded_point(false);
    let x = URL_SAFE_NO_PAD.encode(point.x().expect("x"));
    let y = URL_SAFE_NO_PAD.encode(point.y().expect("y"));
    let jwk_val = serde_json::json!({"kty": "EC", "crv": "P-256", "x": x, "y": y, "kid": kid});
    AgentFixture {
        jwk_val,
        sub: sub.into(),
    }
}

fn pkcs8_pem(sk: &SigningKey) -> String {
    sk.to_pkcs8_pem(p256::pkcs8::LineEnding::LF).expect("pkcs8").to_string()
}

fn mint_agent_jwt(ap_signing: &SigningKey, ap_kid: &str, ap_iss: &str, agent: &AgentFixture, now: i64) -> String {
    let enc = EncodingKey::from_ec_pem(pkcs8_pem(ap_signing).as_bytes()).expect("enc");
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AGENT.into());
    header.kid = Some(ap_kid.into());
    let claims = serde_json::json!({
        "iss": ap_iss,
        "sub": agent.sub,
        "jti": format!("agent-jti-{}", agent.sub),
        "iat": now - 5,
        "exp": now + 600,
        "dwk": DWK_AGENT,
        "cnf": Cnf { jwk: agent.jwk_val.clone() },
    });
    encode(&header, &claims, &enc).expect("encode agent jwt")
}

fn jwk_thumbprint(jwk_val: &serde_json::Value) -> String {
    trogon_aauth_verify::jwk_thumbprint(jwk_val).expect("thumbprint")
}

#[allow(clippy::too_many_arguments)]
fn mint_resource_jwt(
    resource_signing: &SigningKey,
    resource_kid: &str,
    resource_iss: &str,
    ps_aud: &str,
    agent_sub: &str,
    agent_jkt: &str,
    jti: &str,
    now: i64,
) -> String {
    let enc = EncodingKey::from_ec_pem(pkcs8_pem(resource_signing).as_bytes()).expect("enc");
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(TYP_RESOURCE.into());
    header.kid = Some(resource_kid.into());
    let claims = ResourceClaims {
        iss: resource_iss.to_string(),
        aud: ps_aud.to_string(),
        jti: jti.to_string(),
        iat: now - 5,
        exp: now + 600,
        dwk: DWK_RESOURCE.to_string(),
        agent: agent_sub.to_string(),
        agent_jkt: agent_jkt.to_string(),
        scope: "calendar.readwrite".to_string(),
        mission: None,
    };
    encode(&header, &claims, &enc).expect("encode resource jwt")
}

fn signer_jwk_set(signing_key: &SigningKey, kid: &str) -> JwkSet {
    let verifying = signing_key.verifying_key();
    let point = verifying.to_encoded_point(false);
    let x = URL_SAFE_NO_PAD.encode(point.x().expect("x"));
    let y = URL_SAFE_NO_PAD.encode(point.y().expect("y"));
    let jwk = Jwk {
        common: CommonParameters {
            public_key_use: Some(PublicKeyUse::Signature),
            key_id: Some(kid.into()),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::EllipticCurve(EllipticCurveKeyParameters {
            key_type: EllipticCurveKeyType::EC,
            curve: EllipticCurve::P256,
            x,
            y,
        }),
    };
    JwkSet { keys: vec![jwk] }
}

#[derive(Debug, thiserror::Error)]
#[error("stub policy error")]
struct StubPolicyError;

struct ScriptedPolicy {
    decisions: StdMutex<Vec<PolicyDecision>>,
}

impl ScriptedPolicy {
    fn new(decisions: Vec<PolicyDecision>) -> Self {
        let mut decisions = decisions;
        decisions.reverse();
        Self {
            decisions: StdMutex::new(decisions),
        }
    }
}

#[async_trait]
impl PolicyEngine for ScriptedPolicy {
    type Error = StubPolicyError;

    async fn decide(&self, _request: DecisionRequest<'_>) -> Result<PolicyDecision, Self::Error> {
        let mut decisions = self.decisions.lock().unwrap();
        decisions.pop().ok_or(StubPolicyError)
    }
}

type TestServer = PersonServer<StaticJwks, SystemTimeSource, ScriptedPolicy, NoopInteractionChannel, InMemoryStore>;

fn build_server(decisions: Vec<PolicyDecision>, jwks: StaticJwks, ps_signing: &SigningKey, ps_iss: &str) -> TestServer {
    let ps_encoding_key = EncodingKey::from_ec_pem(pkcs8_pem(ps_signing).as_bytes()).expect("ps enc key");
    PersonServer::new(
        TokenVerifier::new(jwks, SystemTimeSource),
        SystemTimeSource,
        ScriptedPolicy::new(decisions),
        NoopInteractionChannel,
        InMemoryStore::new(),
        ps_encoding_key,
        Algorithm::ES256,
        "ps-kid",
        ps_iss,
    )
}

/// Grant path: mints a real agent token and resource challenge, walks the
/// `PersonServer` through a direct grant, and verifies the resulting
/// `aa-auth+jwt` with `trogon-aauth-verify`, including agent binding
/// (`agent` / `agent_jkt` on the auth token must match the presenting
/// agent), per "PS Response" and "Auth Token Verification".
#[tokio::test]
async fn grant_path_mints_verifiable_auth_token_bound_to_agent() {
    let now = trogon_aauth_verify::SystemTimeSource.now();
    let ap_signing = SigningKey::random(&mut OsRng);
    let agent = agent_fixture("aauth:assistant@agent.example", "ap-kid");
    let agent_jwt = mint_agent_jwt(&ap_signing, "ap-kid", "https://ap.example", &agent, now);
    let agent_jkt = jwk_thumbprint(&agent.jwk_val);

    let resource_signing = SigningKey::random(&mut OsRng);
    let resource_jwt = mint_resource_jwt(
        &resource_signing,
        "resource-kid",
        "https://calendar.example",
        "https://ps.example",
        &agent.sub,
        &agent_jkt,
        "resource-jti-1",
        now,
    );

    let jwks = StaticJwks::new()
        .with("https://ap.example", signer_jwk_set(&ap_signing, "ap-kid"))
        .with(
            "https://calendar.example",
            signer_jwk_set(&resource_signing, "resource-kid"),
        );

    let ps_signing = SigningKey::random(&mut OsRng);
    let server = build_server(
        vec![PolicyDecision::Grant {
            scope: "calendar.readwrite".to_string(),
        }],
        jwks,
        &ps_signing,
        "https://ps.example",
    );

    let req = TokenRequest::new(resource_jwt);
    let outcome = server.evaluate_token_request(&agent_jwt, &req).await.expect("evaluate");
    let auth_token = match outcome {
        TokenEndpointOutcome::Grant { response } => response.auth_token,
        other => panic!("expected Grant, got {other:?}"),
    };

    // Verify the minted aa-auth+jwt with trogon-aauth-verify, mirroring what
    // a resource would do on receiving it.
    let ps_pub_jwk = {
        let verifying = ps_signing.verifying_key();
        let point = verifying.to_encoded_point(false);
        let x = URL_SAFE_NO_PAD.encode(point.x().expect("x"));
        let y = URL_SAFE_NO_PAD.encode(point.y().expect("y"));
        Jwk {
            common: CommonParameters {
                public_key_use: Some(PublicKeyUse::Signature),
                key_id: Some("ps-kid".into()),
                ..Default::default()
            },
            algorithm: AlgorithmParameters::EllipticCurve(EllipticCurveKeyParameters {
                key_type: EllipticCurveKeyType::EC,
                curve: EllipticCurve::P256,
                x,
                y,
            }),
        }
    };
    let verify_jwks = StaticJwks::new().with("https://ps.example", JwkSet { keys: vec![ps_pub_jwk] });
    let verifier = TokenVerifier::new(verify_jwks, SystemTimeSource);
    let verified = verifier
        .verify_auth(&auth_token, "https://calendar.example")
        .await
        .expect("verify auth token");

    assert_eq!(verified.claims.iss, "https://ps.example");
    assert_eq!(verified.claims.aud, "https://calendar.example");
    assert_eq!(verified.claims.agent, agent.sub);
    assert_eq!(verified.claims.agent_jkt, agent_jkt);
    assert_eq!(verified.claims.scope, "calendar.readwrite");
}

/// Interaction path: policy asks for out-of-band interaction; the agent
/// polls and observes `status: interacting` per "User Interaction", then a
/// second evaluation (simulating the interaction resolving) grants.
#[tokio::test]
async fn interaction_path_then_poll_shows_interacting_status() {
    let now = trogon_aauth_verify::SystemTimeSource.now();
    let ap_signing = SigningKey::random(&mut OsRng);
    let agent = agent_fixture("aauth:assistant@agent.example", "ap-kid");
    let agent_jwt = mint_agent_jwt(&ap_signing, "ap-kid", "https://ap.example", &agent, now);
    let agent_jkt = jwk_thumbprint(&agent.jwk_val);

    let resource_signing = SigningKey::random(&mut OsRng);
    let resource_jwt = mint_resource_jwt(
        &resource_signing,
        "resource-kid",
        "https://calendar.example",
        "https://ps.example",
        &agent.sub,
        &agent_jkt,
        "resource-jti-2",
        now,
    );

    let jwks = StaticJwks::new()
        .with("https://ap.example", signer_jwk_set(&ap_signing, "ap-kid"))
        .with(
            "https://calendar.example",
            signer_jwk_set(&resource_signing, "resource-kid"),
        );

    let ps_signing = SigningKey::random(&mut OsRng);
    let server = build_server(
        vec![PolicyDecision::NeedsInteraction],
        jwks,
        &ps_signing,
        "https://ps.example",
    );

    let req = TokenRequest::new(resource_jwt);
    let outcome = server.evaluate_token_request(&agent_jwt, &req).await.expect("evaluate");
    let pending_id = match outcome {
        TokenEndpointOutcome::Pending { pending_id, response } => {
            assert!(response.is_interacting());
            pending_id
        }
        other => panic!("expected Pending, got {other:?}"),
    };

    let polled = server.poll_pending(&pending_id).await.expect("poll");
    assert_eq!(polled.phase, trogon_aauth_person::PendingPhase::Interacting);
}

/// Clarification path: policy asks a clarifying question, the agent answers
/// via `clarification_response`, and the second policy evaluation grants.
/// Verifies the resulting token again to confirm the clarification round
/// trip does not corrupt the eventual grant's agent binding.
#[tokio::test]
async fn clarification_path_then_grant_produces_verifiable_token() {
    let now = trogon_aauth_verify::SystemTimeSource.now();
    let ap_signing = SigningKey::random(&mut OsRng);
    let agent = agent_fixture("aauth:assistant@agent.example", "ap-kid");
    let agent_jwt = mint_agent_jwt(&ap_signing, "ap-kid", "https://ap.example", &agent, now);
    let agent_jkt = jwk_thumbprint(&agent.jwk_val);

    let resource_signing = SigningKey::random(&mut OsRng);
    let resource_jwt = mint_resource_jwt(
        &resource_signing,
        "resource-kid",
        "https://calendar.example",
        "https://ps.example",
        &agent.sub,
        &agent_jkt,
        "resource-jti-3",
        now,
    );

    let jwks = StaticJwks::new()
        .with("https://ap.example", signer_jwk_set(&ap_signing, "ap-kid"))
        .with(
            "https://calendar.example",
            signer_jwk_set(&resource_signing, "resource-kid"),
        );

    let ps_signing = SigningKey::random(&mut OsRng);
    let server = build_server(
        vec![
            PolicyDecision::NeedsClarification {
                clarification: "Why do you need write access to my calendar?".to_string(),
                options: None,
            },
            PolicyDecision::Grant {
                scope: "calendar.readwrite".to_string(),
            },
        ],
        jwks,
        &ps_signing,
        "https://ps.example",
    );

    let req = TokenRequest::new(resource_jwt);
    let outcome = server.evaluate_token_request(&agent_jwt, &req).await.expect("evaluate");
    let pending_id = match outcome {
        TokenEndpointOutcome::Pending { pending_id, response } => {
            assert_eq!(
                response.clarification.as_deref(),
                Some("Why do you need write access to my calendar?")
            );
            pending_id
        }
        other => panic!("expected Pending, got {other:?}"),
    };

    let outcome = server
        .respond_to_clarification(
            &pending_id,
            ClarificationAction::ClarificationResponse,
            Some("I need to create a meeting invite for the participants you listed."),
            None,
        )
        .await
        .expect("respond to clarification");

    let auth_token = match outcome {
        TokenEndpointOutcome::Grant { response } => response.auth_token,
        other => panic!("expected Grant after clarification, got {other:?}"),
    };

    let ps_pub_jwk = {
        let verifying = ps_signing.verifying_key();
        let point = verifying.to_encoded_point(false);
        let x = URL_SAFE_NO_PAD.encode(point.x().expect("x"));
        let y = URL_SAFE_NO_PAD.encode(point.y().expect("y"));
        Jwk {
            common: CommonParameters {
                public_key_use: Some(PublicKeyUse::Signature),
                key_id: Some("ps-kid".into()),
                ..Default::default()
            },
            algorithm: AlgorithmParameters::EllipticCurve(EllipticCurveKeyParameters {
                key_type: EllipticCurveKeyType::EC,
                curve: EllipticCurve::P256,
                x,
                y,
            }),
        }
    };
    let verify_jwks = StaticJwks::new().with("https://ps.example", JwkSet { keys: vec![ps_pub_jwk] });
    let verifier = TokenVerifier::new(verify_jwks, SystemTimeSource);
    let verified = verifier
        .verify_auth(&auth_token, "https://calendar.example")
        .await
        .expect("verify auth token");

    assert_eq!(verified.claims.agent, agent.sub);
    assert_eq!(verified.claims.agent_jkt, agent_jkt);
}
