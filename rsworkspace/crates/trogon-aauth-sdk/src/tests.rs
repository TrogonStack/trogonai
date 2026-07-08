#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

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
use trogon_aauth_verify::replay::InMemoryReplayStore;
use trogon_aauth_verify::time_source::SystemTimeSource;
use trogon_aauth_verify::{NatsHeaders, NatsPopVerifier, NatsRequest, StaticJwks};
use trogon_identity_types::aauth::{Cnf, DWK_AGENT, TYP_AGENT};

use crate::error::AgentSignerError;
use crate::{AgentSigner, Requirement, parse_requirement};

const AP_ISS: &str = "https://ap.test";
const AP_KID: &str = "ap-key-1";
const AGENT_SUB: &str = "agent-1";

fn pkcs8_pem(sk: &SigningKey) -> String {
    sk.to_pkcs8_pem(p256::pkcs8::LineEnding::LF).expect("pkcs8").to_string()
}

/// Mints a real `aa-agent+jwt` for `agent_signing`'s public key, signed by a
/// throwaway Agent Provider key. Mirrors
/// `a2a-gateway/tests/aauth_roundtrip.rs::mint_agent_jwt`.
fn mint_agent_jwt(ap_signing: &SigningKey, agent_jwk_val: &serde_json::Value, now: i64) -> String {
    let enc = EncodingKey::from_ec_pem(pkcs8_pem(ap_signing).as_bytes()).expect("enc");
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some(TYP_AGENT.into());
    header.kid = Some(AP_KID.into());
    let claims = serde_json::json!({
        "iss": AP_ISS,
        "sub": AGENT_SUB,
        "jti": "agent-jti-1",
        "iat": now - 5,
        "exp": now + 600,
        "dwk": DWK_AGENT,
        "cnf": Cnf { jwk: agent_jwk_val.clone() },
    });
    encode(&header, &claims, &enc).expect("encode agent jwt")
}

fn ap_public_jwk(ap_signing: &SigningKey) -> Jwk {
    let point = ap_signing.verifying_key().to_encoded_point(false);
    let x = URL_SAFE_NO_PAD.encode(point.x().expect("x"));
    let y = URL_SAFE_NO_PAD.encode(point.y().expect("y"));
    Jwk {
        common: CommonParameters {
            public_key_use: Some(PublicKeyUse::Signature),
            key_id: Some(AP_KID.into()),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::EllipticCurve(EllipticCurveKeyParameters {
            key_type: EllipticCurveKeyType::EC,
            curve: EllipticCurve::P256,
            x,
            y,
        }),
    }
}

fn verifier(ap_signing: &SigningKey) -> NatsPopVerifier<StaticJwks, SystemTimeSource, InMemoryReplayStore> {
    let jwks = StaticJwks::new().with(
        AP_ISS.to_string(),
        JwkSet {
            keys: vec![ap_public_jwk(ap_signing)],
        },
    );
    NatsPopVerifier::new(jwks, SystemTimeSource, InMemoryReplayStore::new())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(1_700_000_000)
}

#[tokio::test(flavor = "current_thread")]
async fn roundtrip_signed_request_verifies_and_matches_agent_and_jkt() {
    let ap_signing = SigningKey::random(&mut OsRng);
    let agent_signing = SigningKey::random(&mut OsRng);

    let now = now_unix();

    // Build the AgentSigner first so its derived public JWK is exactly what
    // gets embedded in the aa-agent+jwt's cnf claim.
    let signer = AgentSigner::new(agent_signing.clone(), "placeholder").expect("signer");
    let agent_jwt = mint_agent_jwt(&ap_signing, signer.public_jwk(), now);
    let signer = AgentSigner::new(agent_signing, agent_jwt).expect("signer");

    let subject = "a2a.gateway.bot.message.send";
    let reply = "_INBOX.x.1";
    let payload = b"{\"hello\":\"world\"}".to_vec();

    let headers = signer.sign_nats_request(subject, Some(reply), &payload, now, "nonce-1");

    let verifier = verifier(&ap_signing);
    let pairs = headers.into_pairs();
    let req = NatsRequest {
        subject,
        reply: Some(reply),
        payload: &payload,
        headers: NatsHeaders::new_checked(&pairs).expect("no dup headers"),
    };

    let verified = verifier.verify(&req).await.expect("signature verifies");
    assert_eq!(verified.claims.sub, AGENT_SUB);
    assert_eq!(verified.jkt, signer.jkt());
}

#[tokio::test(flavor = "current_thread")]
async fn tampered_payload_after_signing_is_rejected() {
    let ap_signing = SigningKey::random(&mut OsRng);
    let agent_signing = SigningKey::random(&mut OsRng);
    let now = now_unix();

    let signer = AgentSigner::new(agent_signing.clone(), "placeholder").expect("signer");
    let agent_jwt = mint_agent_jwt(&ap_signing, signer.public_jwk(), now);
    let signer = AgentSigner::new(agent_signing, agent_jwt).expect("signer");

    let subject = "a2a.gateway.bot.message.send";
    let reply = "_INBOX.x.1";
    let original_payload = b"{\"hello\":\"world\"}".to_vec();

    let headers = signer.sign_nats_request(subject, Some(reply), &original_payload, now, "nonce-2");

    let verifier = verifier(&ap_signing);
    let pairs = headers.into_pairs();

    let tampered_payload = b"{\"hello\":\"mallory\"}".to_vec();
    let req = NatsRequest {
        subject,
        reply: Some(reply),
        payload: &tampered_payload,
        headers: NatsHeaders::new_checked(&pairs).expect("no dup headers"),
    };

    let err = verifier
        .verify(&req)
        .await
        .expect_err("tampered payload must not verify");
    assert!(matches!(err, trogon_aauth_verify::NatsPopError::DigestMismatch));
}

#[tokio::test(flavor = "current_thread")]
async fn with_auth_token_adds_auth_header_and_still_verifies() {
    let ap_signing = SigningKey::random(&mut OsRng);
    let agent_signing = SigningKey::random(&mut OsRng);
    let now = now_unix();

    let signer = AgentSigner::new(agent_signing.clone(), "placeholder").expect("signer");
    let agent_jwt = mint_agent_jwt(&ap_signing, signer.public_jwk(), now);
    let signer = AgentSigner::new(agent_signing, agent_jwt)
        .expect("signer")
        .with_auth_token("aa-auth-jwt-opaque-value");

    let subject = "a2a.gateway.bot.message.send";
    let payload = b"{}".to_vec();
    let headers = signer.sign_nats_request(subject, None, &payload, now, "nonce-3");
    let pairs = headers.into_pairs();

    let auth_header = pairs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(trogon_identity_types::aauth::headers::NATS_AUTH_TOKEN))
        .expect("auth token header present");
    assert_eq!(auth_header.1, "aa-auth-jwt-opaque-value");

    let verifier = verifier(&ap_signing);
    let req = NatsRequest {
        subject,
        reply: None,
        payload: &payload,
        headers: NatsHeaders::new_checked(&pairs).expect("no dup headers"),
    };
    verifier
        .verify(&req)
        .await
        .expect("still verifies with auth header present");
}

#[test]
fn requirement_auth_token_roundtrips_through_header_value() {
    let requirement = Requirement::AuthToken {
        resource_token: "eyJhbGciOi...resource-token".to_string(),
    };
    let header_value = requirement.to_header_value();
    let parsed = parse_requirement(&header_value);
    assert_eq!(parsed, requirement);
}

#[test]
fn requirement_interaction_roundtrips_through_header_value() {
    let requirement = Requirement::Interaction {
        url: "https://person.example/consent/abc".to_string(),
        code: Some("XYZ-123".to_string()),
    };
    let header_value = requirement.to_header_value();
    let parsed = parse_requirement(&header_value);
    assert_eq!(parsed, requirement);
}

#[test]
fn sign_nats_request_now_produces_headers_verifiable_shortly_after() {
    let ap_signing = SigningKey::random(&mut OsRng);
    let agent_signing = SigningKey::random(&mut OsRng);
    let now = now_unix();

    let signer = AgentSigner::new(agent_signing.clone(), "placeholder").expect("signer");
    let agent_jwt = mint_agent_jwt(&ap_signing, signer.public_jwk(), now);
    let signer = AgentSigner::new(agent_signing, agent_jwt).expect("signer");

    let payload = b"{}".to_vec();
    let headers_a = signer.sign_nats_request_now("subj", None, &payload);
    let headers_b = signer.sign_nats_request_now("subj", None, &payload);

    // Nonces must differ between calls so replay protection has something to
    // key on, even for identical subject/payload signed back-to-back.
    let nonce_of = |pairs: &[(String, String)]| {
        pairs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(trogon_identity_types::aauth::headers::NATS_SIG_NONCE))
            .map(|(_, v)| v.clone())
            .expect("nonce header present")
    };
    assert_ne!(nonce_of(headers_a.as_pairs()), nonce_of(headers_b.as_pairs()));
}

#[test]
fn from_pkcs8_pem_builds_a_signer_that_matches_the_in_memory_key() {
    let signing_key = SigningKey::random(&mut OsRng);
    let pem = pkcs8_pem(&signing_key);

    let from_pem = AgentSigner::from_pkcs8_pem(&pem, "placeholder").expect("valid pkcs8 pem parses");
    let from_key = AgentSigner::new(signing_key, "placeholder").expect("signer");

    assert_eq!(from_pem.public_jwk(), from_key.public_jwk());
    assert_eq!(from_pem.jkt(), from_key.jkt());
}

#[test]
fn from_pkcs8_pem_rejects_a_malformed_pem() {
    let result = AgentSigner::from_pkcs8_pem("not a pem at all", "placeholder");
    assert!(matches!(result, Err(AgentSignerError::InvalidPkcs8(_))));
}
