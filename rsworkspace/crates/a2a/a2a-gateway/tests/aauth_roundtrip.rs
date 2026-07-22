//! End-to-end integration test for `AAuthIngress::resolve_nats`.
//!
//! Mints a real `aa-agent+jwt`, signs a NATS PoP envelope with a real P-256
//! key, and drives `resolve_nats` through:
//! - happy path (agent + matching auth token → `AAuthResolution`)
//! - agent mismatch (auth token claims a different agent → `AuthAgentMismatch`
//!   deny, with the challenge bound to the PoP-verified agent)

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_arguments)]

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, EllipticCurve, EllipticCurveKeyParameters, EllipticCurveKeyType, Jwk,
    JwkSet, PublicKeyUse,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::EncodePrivateKey;
use rand_core::OsRng;
use trogon_aauth_verify::nats_pop::content_digest_sha256;
use trogon_identity_types::aauth::{Cnf, DWK_AGENT, DWK_RESOURCE, NatsSignatureEnvelope, TYP_AGENT, TYP_AUTH, headers};

use a2a_gateway::aauth::{
    AAUTH_REQUIRED_CODE, AAuthConfig, AAuthDenyReasonError, AAuthIngress, AAuthMode, ChallengeKid, LeewaySecs,
    NonNegativeSecs, PersonServerAudience, ResourceIssuer, StaticJwks,
};

struct AgentFixture {
    signing_key: SigningKey,
    jwk_val: serde_json::Value,
    jwk: Jwk,
    sub: String,
}

fn agent_fixture(sub: &str, kid: &str) -> AgentFixture {
    let signing_key = SigningKey::random(&mut OsRng);
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
            x: x.clone(),
            y: y.clone(),
        }),
    };
    let jwk_val = serde_json::json!({"kty": "EC", "crv": "P-256", "x": x, "y": y, "kid": kid});
    AgentFixture {
        signing_key,
        jwk_val,
        jwk,
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

fn mint_auth_jwt(
    ap_signing: &SigningKey,
    ap_kid: &str,
    ap_iss: &str,
    resource_iss: &str,
    agent_sub: &str,
    agent_jkt: &str,
    principal: &str,
    now: i64,
) -> String {
    mint_auth_jwt_with(
        ap_signing,
        ap_kid,
        ap_iss,
        resource_iss,
        agent_sub,
        agent_jkt,
        principal,
        "*",
        None,
        now,
    )
}

#[allow(clippy::too_many_arguments)]
fn mint_auth_jwt_with(
    ap_signing: &SigningKey,
    ap_kid: &str,
    ap_iss: &str,
    resource_iss: &str,
    agent_sub: &str,
    agent_jkt: &str,
    principal: &str,
    scope: &str,
    mission: Option<(&str, &str)>,
    now: i64,
) -> String {
    let enc = EncodingKey::from_ec_pem(pkcs8_pem(ap_signing).as_bytes()).expect("enc");
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AUTH.into());
    header.kid = Some(ap_kid.into());
    let mut claims = serde_json::json!({
        "iss": ap_iss,
        "sub": agent_sub,
        "aud": resource_iss,
        "jti": format!("auth-jti-{agent_sub}"),
        "iat": now - 5,
        "exp": now + 600,
        "agent": agent_sub,
        "agent_jkt": agent_jkt,
        "scope": scope,
        "principal": principal,
    });
    if let Some((approver, s256)) = mission {
        claims["mission"] = serde_json::json!({ "approver": approver, "s256": s256 });
    }
    encode(&header, &claims, &enc).expect("encode auth jwt")
}

fn pop_headers(
    agent: &AgentFixture,
    agent_jwt: &str,
    subject: &str,
    reply: &str,
    payload: &[u8],
    now: i64,
) -> Vec<(String, String)> {
    let digest = content_digest_sha256(payload);
    let nonce = "n-1";
    let sig_input =
        "(\"@subject\" \"@reply\" \"content-digest\" \"aauth-token\" \"aauth-sig-created\" \"aauth-sig-nonce\")";
    let jkt = trogon_aauth_verify::jwk_thumbprint(&agent.jwk_val).expect("jkt");
    let envelope = NatsSignatureEnvelope {
        token: agent_jwt.to_string(),
        sig_input: sig_input.to_string(),
        sig: String::new(),
        created: now,
        nonce: nonce.into(),
        content_digest: digest.clone(),
    };
    let canonical = envelope.canonical_base(subject, Some(reply), &jkt);
    let sig: Signature = agent.signing_key.sign(canonical.as_bytes());
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig.to_bytes());
    vec![
        (headers::NATS_TOKEN.to_string(), agent_jwt.to_string()),
        (headers::NATS_SIG_INPUT.to_string(), sig_input.into()),
        (headers::NATS_SIG.to_string(), sig_b64),
        (headers::NATS_SIG_CREATED.to_string(), now.to_string()),
        (headers::NATS_SIG_NONCE.to_string(), nonce.into()),
        (headers::CONTENT_DIGEST.to_string(), digest),
    ]
}

fn build_ingress(
    ap_jwk: Jwk,
    ap_iss: &str,
    resource_iss: &str,
    mode: AAuthMode,
) -> AAuthIngress<StaticJwks, trogon_aauth_verify::InMemoryReplayStore> {
    let jwks = StaticJwks::new().with(ap_iss.to_string(), JwkSet { keys: vec![ap_jwk] });
    // Reuse the AP signing key as the gateway's challenge signer so this
    // test doesn't have to manage a second EC key — the challenge is
    // self-signed by the gateway and won't be verified by the test.
    let challenge_key =
        EncodingKey::from_ec_pem(pkcs8_pem(&SigningKey::random(&mut OsRng)).as_bytes()).expect("challenge enc");
    AAuthIngress::new_in_memory(AAuthConfig {
        mode,
        jwks,
        resource_iss: ResourceIssuer::new(resource_iss).expect("resource iss"),
        person_server_aud: PersonServerAudience::new(resource_iss).expect("aud"),
        leeway_secs: LeewaySecs::new(30),
        challenge_alg: Algorithm::ES256,
        challenge_key,
        challenge_kid: ChallengeKid::new("gw-kid").expect("kid"),
        challenge_ttl_secs: NonNegativeSecs::new(60).expect("ttl"),
        max_skew_secs: NonNegativeSecs::new(60).expect("skew"),
    })
}

#[tokio::test(flavor = "current_thread")]
async fn resolve_nats_happy_path_with_matching_auth_token() {
    let ap_signing = SigningKey::random(&mut OsRng);
    let ap_verify = ap_signing.verifying_key();
    let ap = agent_fixture("__ap__", "ap-key-1");
    // Override the fixture's signing key with the real AP key; we only use
    // the fixture struct to derive the JWK shape uniformly.
    let ap_jwk_val = ap.jwk_val.clone();
    let ap_jwk = ap.jwk.clone();
    let _ = ap_verify;
    let _ = ap_jwk_val;

    let agent = agent_fixture("agent-1", "agent-key-1");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(1_700_000_000);

    let ap_iss = "https://ap.test";
    let resource_iss = "https://resource.test";

    let agent_jwt = mint_agent_jwt(&ap_signing, "ap-key-1", ap_iss, &agent, now);
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent.jwk_val).expect("jkt");
    let auth_jwt = mint_auth_jwt(
        &ap_signing,
        "ap-key-1",
        ap_iss,
        resource_iss,
        &agent.sub,
        &agent_jkt,
        "spicedb-principal",
        now,
    );

    // Build JWKS keyed by ap_iss with the AP's public key.
    let ap_public_jwk = {
        let point = ap_signing.verifying_key().to_encoded_point(false);
        let x = URL_SAFE_NO_PAD.encode(point.x().expect("x"));
        let y = URL_SAFE_NO_PAD.encode(point.y().expect("y"));
        Jwk {
            common: CommonParameters {
                public_key_use: Some(PublicKeyUse::Signature),
                key_id: Some("ap-key-1".into()),
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
    let _ = ap_jwk;
    let ingress = Arc::new(build_ingress(ap_public_jwk, ap_iss, resource_iss, AAuthMode::Enforce));

    let subject = "a2a.gateway.bot.message.send";
    let reply = "_INBOX.x.1";
    let payload = b"{}".to_vec();
    let pop = pop_headers(&agent, &agent_jwt, subject, reply, &payload, now);

    let res = ingress
        .resolve_nats(subject, Some(reply), &payload, &pop, Some(&auth_jwt), "message.send")
        .await
        .expect("agent + matching auth verifies");
    assert_eq!(res.agent_id.as_deref(), Some("agent-1"));
    assert_eq!(res.agent_jkt.as_deref(), Some(agent_jkt.as_str()));
    assert_eq!(res.principal.as_deref(), Some("spicedb-principal"));
}

#[tokio::test(flavor = "current_thread")]
async fn resolve_nats_rejects_auth_token_bound_to_different_agent() {
    let ap_signing = SigningKey::random(&mut OsRng);
    let agent = agent_fixture("agent-A", "agent-key-A");
    let other = agent_fixture("agent-B", "agent-key-B");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(1_700_000_000);

    let ap_iss = "https://ap.test";
    let resource_iss = "https://resource.test";

    // PoP-presenting agent.
    let agent_jwt = mint_agent_jwt(&ap_signing, "ap-key-1", ap_iss, &agent, now);
    // Auth token bound to a different agent (other).
    let other_jkt = trogon_aauth_verify::jwk_thumbprint(&other.jwk_val).expect("jkt");
    let mismatched_auth = mint_auth_jwt(
        &ap_signing,
        "ap-key-1",
        ap_iss,
        resource_iss,
        &other.sub,
        &other_jkt,
        "spicedb-other",
        now,
    );

    let ap_public_jwk = {
        let point = ap_signing.verifying_key().to_encoded_point(false);
        let x = URL_SAFE_NO_PAD.encode(point.x().expect("x"));
        let y = URL_SAFE_NO_PAD.encode(point.y().expect("y"));
        Jwk {
            common: CommonParameters {
                public_key_use: Some(PublicKeyUse::Signature),
                key_id: Some("ap-key-1".into()),
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
    let ingress = build_ingress(ap_public_jwk, ap_iss, resource_iss, AAuthMode::Enforce);

    let subject = "a2a.gateway.bot.message.send";
    let reply = "_INBOX.x.1";
    let payload = b"{}".to_vec();
    let pop = pop_headers(&agent, &agent_jwt, subject, reply, &payload, now);

    let err = ingress
        .resolve_nats(
            subject,
            Some(reply),
            &payload,
            &pop,
            Some(&mismatched_auth),
            "message.send",
        )
        .await
        .expect_err("agent mismatch denies");
    assert_eq!(err.code, AAUTH_REQUIRED_CODE);
    assert!(matches!(err.reason, AAuthDenyReasonError::AuthAgentMismatch { .. }));
    // Challenge IS issued because the PoP-verified agent had a jkt to bind to.
    assert!(err.challenge.is_some(), "expected challenge bound to verified agent");
}

#[tokio::test(flavor = "current_thread")]
async fn resolve_nats_enforce_mode_denies_scope_not_covering_method() {
    let ap_signing = SigningKey::random(&mut OsRng);
    let agent = agent_fixture("agent-1", "agent-key-1");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(1_700_000_000);

    let ap_iss = "https://ap.test";
    let resource_iss = "https://resource.test";

    let agent_jwt = mint_agent_jwt(&ap_signing, "ap-key-1", ap_iss, &agent, now);
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent.jwk_val).expect("jkt");
    // Scope only covers `tasks.*`, not the `message.send` method invoked below.
    let auth_jwt = mint_auth_jwt_with(
        &ap_signing,
        "ap-key-1",
        ap_iss,
        resource_iss,
        &agent.sub,
        &agent_jkt,
        "spicedb-principal",
        "tasks.*",
        None,
        now,
    );

    let ap_public_jwk = {
        let point = ap_signing.verifying_key().to_encoded_point(false);
        let x = URL_SAFE_NO_PAD.encode(point.x().expect("x"));
        let y = URL_SAFE_NO_PAD.encode(point.y().expect("y"));
        Jwk {
            common: CommonParameters {
                public_key_use: Some(PublicKeyUse::Signature),
                key_id: Some("ap-key-1".into()),
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
    let ingress = build_ingress(ap_public_jwk, ap_iss, resource_iss, AAuthMode::Enforce);

    let subject = "a2a.gateway.bot.message.send";
    let reply = "_INBOX.x.1";
    let payload = b"{}".to_vec();
    let pop = pop_headers(&agent, &agent_jwt, subject, reply, &payload, now);

    let err = ingress
        .resolve_nats(subject, Some(reply), &payload, &pop, Some(&auth_jwt), "message.send")
        .await
        .expect_err("uncovered scope denies in enforce mode");
    assert_eq!(err.code, AAUTH_REQUIRED_CODE);
    assert!(matches!(err.reason, AAuthDenyReasonError::ScopeNotCovered { .. }));
    assert!(err.challenge.is_some(), "expected fresh auth-token challenge");
}

#[tokio::test(flavor = "current_thread")]
async fn resolve_nats_mission_header_matches_auth_claim() {
    let ap_signing = SigningKey::random(&mut OsRng);
    let agent = agent_fixture("agent-1", "agent-key-1");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(1_700_000_000);

    let ap_iss = "https://ap.test";
    let resource_iss = "https://resource.test";

    let agent_jwt = mint_agent_jwt(&ap_signing, "ap-key-1", ap_iss, &agent, now);
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent.jwk_val).expect("jkt");
    let mission_approver = "approver-1";
    let mission_s256 = "a".repeat(64);
    let auth_jwt = mint_auth_jwt_with(
        &ap_signing,
        "ap-key-1",
        ap_iss,
        resource_iss,
        &agent.sub,
        &agent_jkt,
        "spicedb-principal",
        "*",
        Some((mission_approver, mission_s256.as_str())),
        now,
    );

    let ap_public_jwk = {
        let point = ap_signing.verifying_key().to_encoded_point(false);
        let x = URL_SAFE_NO_PAD.encode(point.x().expect("x"));
        let y = URL_SAFE_NO_PAD.encode(point.y().expect("y"));
        Jwk {
            common: CommonParameters {
                public_key_use: Some(PublicKeyUse::Signature),
                key_id: Some("ap-key-1".into()),
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
    let ingress = build_ingress(ap_public_jwk, ap_iss, resource_iss, AAuthMode::Enforce);

    let subject = "a2a.gateway.bot.message.send";
    let reply = "_INBOX.x.1";
    let payload = b"{}".to_vec();
    let mut pop = pop_headers(&agent, &agent_jwt, subject, reply, &payload, now);
    let mission_header = trogon_identity_types::aauth::mission::MissionHeader {
        approver: mission_approver.to_string(),
        s256: mission_s256.clone(),
    };
    pop.push((headers::MISSION.to_string(), mission_header.to_header_value()));

    let res = ingress
        .resolve_nats(subject, Some(reply), &payload, &pop, Some(&auth_jwt), "message.send")
        .await
        .expect("matching mission header verifies");
    assert_eq!(res.mission_approver.as_deref(), Some(mission_approver));
}

#[tokio::test(flavor = "current_thread")]
async fn resolve_nats_denies_mission_claim_without_header() {
    let ap_signing = SigningKey::random(&mut OsRng);
    let agent = agent_fixture("agent-1", "agent-key-1");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(1_700_000_000);

    let ap_iss = "https://ap.test";
    let resource_iss = "https://resource.test";

    let agent_jwt = mint_agent_jwt(&ap_signing, "ap-key-1", ap_iss, &agent, now);
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent.jwk_val).expect("jkt");
    let mission_s256 = "a".repeat(64);
    let auth_jwt = mint_auth_jwt_with(
        &ap_signing,
        "ap-key-1",
        ap_iss,
        resource_iss,
        &agent.sub,
        &agent_jkt,
        "spicedb-principal",
        "*",
        Some(("approver-1", mission_s256.as_str())),
        now,
    );

    let ap_public_jwk = {
        let point = ap_signing.verifying_key().to_encoded_point(false);
        let x = URL_SAFE_NO_PAD.encode(point.x().expect("x"));
        let y = URL_SAFE_NO_PAD.encode(point.y().expect("y"));
        Jwk {
            common: CommonParameters {
                public_key_use: Some(PublicKeyUse::Signature),
                key_id: Some("ap-key-1".into()),
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
    let ingress = build_ingress(ap_public_jwk, ap_iss, resource_iss, AAuthMode::Enforce);

    let subject = "a2a.gateway.bot.message.send";
    let reply = "_INBOX.x.1";
    let payload = b"{}".to_vec();
    // No AAuth-Mission header on the request: a mission-scoped token must
    // not be usable outside its mission context.
    let pop = pop_headers(&agent, &agent_jwt, subject, reply, &payload, now);

    let err = ingress
        .resolve_nats(subject, Some(reply), &payload, &pop, Some(&auth_jwt), "message.send")
        .await
        .expect_err("mission claim without header denies");
    assert_eq!(err.code, AAUTH_REQUIRED_CODE);
    assert!(matches!(
        err.reason,
        AAuthDenyReasonError::MissionHeaderMissing { ref approver } if approver == "approver-1"
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn resolve_nats_denies_malformed_mission_claim() {
    let ap_signing = SigningKey::random(&mut OsRng);
    let agent = agent_fixture("agent-1", "agent-key-1");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(1_700_000_000);

    let ap_iss = "https://ap.test";
    let resource_iss = "https://resource.test";

    let agent_jwt = mint_agent_jwt(&ap_signing, "ap-key-1", ap_iss, &agent, now);
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent.jwk_val).expect("jkt");

    // A `mission` claim that is not `{approver, s256}` must deny rather than
    // silently degrade the token to an unscoped grant.
    let enc = EncodingKey::from_ec_pem(pkcs8_pem(&ap_signing).as_bytes()).expect("enc");
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AUTH.into());
    header.kid = Some("ap-key-1".into());
    let claims = serde_json::json!({
        "iss": ap_iss,
        "sub": agent.sub,
        "aud": resource_iss,
        "jti": "auth-jti-malformed-mission",
        "iat": now - 5,
        "exp": now + 600,
        "agent": agent.sub,
        "agent_jkt": agent_jkt,
        "scope": "*",
        "principal": "spicedb-principal",
        "mission": "not-an-object",
    });
    let auth_jwt = encode(&header, &claims, &enc).expect("encode auth jwt");

    let ap_public_jwk = {
        let point = ap_signing.verifying_key().to_encoded_point(false);
        let x = URL_SAFE_NO_PAD.encode(point.x().expect("x"));
        let y = URL_SAFE_NO_PAD.encode(point.y().expect("y"));
        Jwk {
            common: CommonParameters {
                public_key_use: Some(PublicKeyUse::Signature),
                key_id: Some("ap-key-1".into()),
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
    let ingress = build_ingress(ap_public_jwk, ap_iss, resource_iss, AAuthMode::Enforce);

    let subject = "a2a.gateway.bot.message.send";
    let reply = "_INBOX.x.1";
    let payload = b"{}".to_vec();
    let pop = pop_headers(&agent, &agent_jwt, subject, reply, &payload, now);

    let err = ingress
        .resolve_nats(subject, Some(reply), &payload, &pop, Some(&auth_jwt), "message.send")
        .await
        .expect_err("malformed mission claim denies");
    assert_eq!(err.code, AAUTH_REQUIRED_CODE);
    assert!(matches!(err.reason, AAuthDenyReasonError::MissionMismatch(_)));
}

#[tokio::test(flavor = "current_thread")]
async fn resolve_nats_denies_expired_auth_token_with_valid_pop() {
    // The PoP envelope verifies (the agent is who it says it is) but the
    // presented `aa-auth+jwt` is expired — verify_auth must fail and the
    // deny must still bind a challenge to the PoP-verified agent.
    let ap_signing = SigningKey::random(&mut OsRng);
    let agent = agent_fixture("agent-1", "agent-key-1");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(1_700_000_000);

    let ap_iss = "https://ap.test";
    let resource_iss = "https://resource.test";

    let agent_jwt = mint_agent_jwt(&ap_signing, "ap-key-1", ap_iss, &agent, now);
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent.jwk_val).expect("jkt");

    // Expired well outside the ingress's configured leeway (30s in
    // build_ingress) so verify_auth's freshness check fails.
    let enc = EncodingKey::from_ec_pem(pkcs8_pem(&ap_signing).as_bytes()).expect("enc");
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AUTH.into());
    header.kid = Some("ap-key-1".into());
    let claims = serde_json::json!({
        "iss": ap_iss,
        "sub": agent.sub,
        "aud": resource_iss,
        "jti": "auth-jti-expired",
        "iat": now - 4000,
        "exp": now - 3600,
        "agent": agent.sub,
        "agent_jkt": agent_jkt,
        "scope": "*",
        "principal": "spicedb-principal",
    });
    let expired_auth_jwt = encode(&header, &claims, &enc).expect("encode expired auth jwt");

    let ap_public_jwk = {
        let point = ap_signing.verifying_key().to_encoded_point(false);
        let x = URL_SAFE_NO_PAD.encode(point.x().expect("x"));
        let y = URL_SAFE_NO_PAD.encode(point.y().expect("y"));
        Jwk {
            common: CommonParameters {
                public_key_use: Some(PublicKeyUse::Signature),
                key_id: Some("ap-key-1".into()),
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
    let ingress = build_ingress(ap_public_jwk, ap_iss, resource_iss, AAuthMode::Enforce);

    let subject = "a2a.gateway.bot.message.send";
    let reply = "_INBOX.x.1";
    let payload = b"{}".to_vec();
    let pop = pop_headers(&agent, &agent_jwt, subject, reply, &payload, now);

    let err = ingress
        .resolve_nats(
            subject,
            Some(reply),
            &payload,
            &pop,
            Some(&expired_auth_jwt),
            "message.send",
        )
        .await
        .expect_err("expired auth token denies");
    assert_eq!(err.code, AAUTH_REQUIRED_CODE);
    assert!(matches!(err.reason, AAuthDenyReasonError::Auth(_)));
    // Challenge IS issued because PoP already verified the presenting agent.
    assert!(err.challenge.is_some(), "expected challenge bound to verified agent");
}

#[tokio::test(flavor = "current_thread")]
async fn resolve_nats_denies_mission_header_mismatching_claim() {
    // Both the header and the claim are present, but they disagree — a
    // mission-scoped grant must not be usable under a header naming a
    // different approver/content hash than the token was minted for.
    let ap_signing = SigningKey::random(&mut OsRng);
    let agent = agent_fixture("agent-1", "agent-key-1");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(1_700_000_000);

    let ap_iss = "https://ap.test";
    let resource_iss = "https://resource.test";

    let agent_jwt = mint_agent_jwt(&ap_signing, "ap-key-1", ap_iss, &agent, now);
    let agent_jkt = trogon_aauth_verify::jwk_thumbprint(&agent.jwk_val).expect("jkt");
    let claim_s256 = "a".repeat(64);
    let auth_jwt = mint_auth_jwt_with(
        &ap_signing,
        "ap-key-1",
        ap_iss,
        resource_iss,
        &agent.sub,
        &agent_jkt,
        "spicedb-principal",
        "*",
        Some(("approver-1", claim_s256.as_str())),
        now,
    );

    let ap_public_jwk = {
        let point = ap_signing.verifying_key().to_encoded_point(false);
        let x = URL_SAFE_NO_PAD.encode(point.x().expect("x"));
        let y = URL_SAFE_NO_PAD.encode(point.y().expect("y"));
        Jwk {
            common: CommonParameters {
                public_key_use: Some(PublicKeyUse::Signature),
                key_id: Some("ap-key-1".into()),
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
    let ingress = build_ingress(ap_public_jwk, ap_iss, resource_iss, AAuthMode::Enforce);

    let subject = "a2a.gateway.bot.message.send";
    let reply = "_INBOX.x.1";
    let payload = b"{}".to_vec();
    let mut pop = pop_headers(&agent, &agent_jwt, subject, reply, &payload, now);
    // Header names a different approver than the claim, so the s256 match
    // alone can't paper over the mismatch.
    let mission_header = trogon_identity_types::aauth::mission::MissionHeader {
        approver: "approver-2".to_string(),
        s256: claim_s256,
    };
    pop.push((headers::MISSION.to_string(), mission_header.to_header_value()));

    let err = ingress
        .resolve_nats(subject, Some(reply), &payload, &pop, Some(&auth_jwt), "message.send")
        .await
        .expect_err("mismatched mission header denies");
    assert_eq!(err.code, AAUTH_REQUIRED_CODE);
    assert!(matches!(err.reason, AAuthDenyReasonError::MissionMismatch(_)));
}

const _: () = {
    // Just reference DWK_RESOURCE so the import stays referenced even when
    // we don't decode the gateway-minted challenge below; keeps the test
    // file self-documenting about which envelope types are involved.
    let _ = DWK_RESOURCE;
};
