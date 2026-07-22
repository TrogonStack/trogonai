//! Mint an `aa-agent+jwt` via `trogon_jwks_publisher::provider` and verify it
//! with `trogon_aauth_verify::TokenVerifier`, the same verifier resources use
//! in production. This replaces the hand-rolled minting pattern in
//! `a2a-gateway/tests/aauth_roundtrip.rs` with the typed provider API.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::EncodingKey;
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, EllipticCurve, EllipticCurveKeyParameters, EllipticCurveKeyType, Jwk,
    JwkSet, PublicKeyUse,
};
use p256::ecdsa::SigningKey;
use p256::pkcs8::EncodePrivateKey;
use rand_core::OsRng;
use trogon_aauth_verify::{StaticJwks, SystemTimeSource, TokenVerifier, jwk_thumbprint};
use trogon_identity_types::aauth::DWK_AGENT;
use trogon_jwks_publisher::provider::{
    AgentIdentifier, AgentProvider, AgentProviderKey, AgentTokenRequest, KeyId, PersonServerUrl, ProviderIssuer,
    TokenTtl,
};

/// Generate a fresh P-256 key pair and return the agent's public JWK
/// (matching `Cnf.jwk`'s `serde_json::Value` shape) alongside the private key.
fn agent_key_pair() -> (serde_json::Value, SigningKey) {
    let signing_key = SigningKey::random(&mut OsRng);
    let point = signing_key.verifying_key().to_encoded_point(false);
    let x = URL_SAFE_NO_PAD.encode(point.x().expect("x coordinate"));
    let y = URL_SAFE_NO_PAD.encode(point.y().expect("y coordinate"));
    (
        serde_json::json!({"kty": "EC", "crv": "P-256", "x": x, "y": y}),
        signing_key,
    )
}

#[tokio::test(flavor = "current_thread")]
async fn mint_then_verify_agent_token_roundtrip() {
    // Agent Provider's own signing key (mints the token).
    let provider_signing_key = SigningKey::random(&mut OsRng);
    let provider_pem = provider_signing_key
        .to_pkcs8_pem(p256::pkcs8::LineEnding::LF)
        .expect("provider pkcs8 pem")
        .to_string();
    let provider_point = provider_signing_key.verifying_key().to_encoded_point(false);
    let provider_x = URL_SAFE_NO_PAD.encode(provider_point.x().expect("x"));
    let provider_y = URL_SAFE_NO_PAD.encode(provider_point.y().expect("y"));
    let provider_kid = "ap-key-1";
    let provider_jwk = Jwk {
        common: CommonParameters {
            public_key_use: Some(PublicKeyUse::Signature),
            key_id: Some(provider_kid.to_string()),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::EllipticCurve(EllipticCurveKeyParameters {
            key_type: EllipticCurveKeyType::EC,
            curve: EllipticCurve::P256,
            x: provider_x,
            y: provider_y,
        }),
    };

    // Agent's own key pair; its public JWK gets embedded into cnf.jwk.
    let (agent_jwk_value, _agent_signing_key) = agent_key_pair();
    let agent_jkt_expected = jwk_thumbprint(&agent_jwk_value).expect("thumbprint");

    let provider_iss = ProviderIssuer::new("https://ap.example").expect("valid iss");
    let key = AgentProviderKey::new(
        EncodingKey::from_ec_pem(provider_pem.as_bytes()).expect("encoding key"),
        KeyId::new(provider_kid).expect("valid kid"),
        provider_iss.clone(),
    );
    let provider = AgentProvider::new(key);

    let sub = AgentIdentifier::new("aauth:assistant-v2@agent.example").expect("valid agent identifier");
    let req = AgentTokenRequest {
        sub: sub.clone(),
        agent_jwk: agent_jwk_value.clone(),
        ttl: TokenTtl::new(3600).expect("positive ttl"),
        ps: Some(PersonServerUrl::new("https://ps.example").expect("valid ps url")),
    };

    let jwt = provider.mint(req).expect("mint aa-agent+jwt");

    let jwks = StaticJwks::new().with(
        provider_iss.as_str(),
        JwkSet {
            keys: vec![provider_jwk],
        },
    );
    let verifier = TokenVerifier::new(jwks, SystemTimeSource);
    let verified = verifier.verify_agent(&jwt).await.expect("verify_agent succeeds");

    assert_eq!(verified.claims.sub, sub.as_str());
    assert_eq!(verified.claims.iss, provider_iss.as_str());
    assert_eq!(verified.claims.dwk, DWK_AGENT);
    assert_eq!(verified.claims.ps.as_deref(), Some("https://ps.example"));
    assert_eq!(verified.jkt, agent_jkt_expected);
}
