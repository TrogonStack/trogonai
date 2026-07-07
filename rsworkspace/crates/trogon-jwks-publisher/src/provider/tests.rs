use super::*;
use base64::Engine as _;

#[test]
fn accepts_valid_top_level_identifier() {
    let id = AgentIdentifier::new("aauth:assistant-v2@agent.example").expect("valid id");
    assert_eq!(id.as_str(), "aauth:assistant-v2@agent.example");
    assert_eq!(id.to_string(), "aauth:assistant-v2@agent.example");
}

#[test]
fn accepts_valid_sub_agent_identifier() {
    let id = AgentIdentifier::new("aauth:planner+7f3c@vendor.example").expect("valid sub-agent id");
    assert_eq!(id.as_str(), "aauth:planner+7f3c@vendor.example");
}

#[test]
fn accepts_valid_identifier_with_dot_in_local() {
    AgentIdentifier::new("aauth:planner.7f3c@vendor.example").expect("valid id with dot in local");
}

#[test]
fn rejects_empty_local() {
    let err = AgentIdentifier::new("aauth:@agent.example").unwrap_err();
    assert!(matches!(err, AgentIdentifierError::EmptyLocal));
}

#[test]
fn rejects_local_over_255_chars() {
    let local = "a".repeat(256);
    let raw = format!("aauth:{local}@agent.example");
    let err = AgentIdentifier::new(raw).unwrap_err();
    assert!(matches!(err, AgentIdentifierError::LocalTooLong(256)));
}

#[test]
fn accepts_local_at_255_chars() {
    let local = "a".repeat(255);
    let raw = format!("aauth:{local}@agent.example");
    AgentIdentifier::new(raw).expect("255 chars is the max allowed, not over it");
}

#[test]
fn rejects_uppercase_in_local() {
    let err = AgentIdentifier::new("aauth:Assistant@agent.example").unwrap_err();
    assert!(matches!(err, AgentIdentifierError::InvalidLocalChar(_)));
}

#[test]
fn rejects_invalid_chars_in_local() {
    let err = AgentIdentifier::new("aauth:assistant!@agent.example").unwrap_err();
    assert!(matches!(err, AgentIdentifierError::InvalidLocalChar(_)));
}

#[test]
fn rejects_missing_at() {
    let err = AgentIdentifier::new("aauth:assistant-agent.example").unwrap_err();
    assert!(matches!(err, AgentIdentifierError::MissingAt(_)));
}

#[test]
fn rejects_missing_scheme() {
    let err = AgentIdentifier::new("assistant@agent.example").unwrap_err();
    assert!(matches!(err, AgentIdentifierError::MissingScheme(_)));
}

#[test]
fn rejects_empty_domain() {
    let err = AgentIdentifier::new("aauth:assistant@").unwrap_err();
    assert!(matches!(err, AgentIdentifierError::EmptyDomain));
}

#[test]
fn rejects_domain_with_empty_label() {
    let err = AgentIdentifier::new("aauth:assistant@a..b").unwrap_err();
    assert!(matches!(err, AgentIdentifierError::InvalidDomain(_)));
}

#[test]
fn rejects_domain_with_leading_dot() {
    let err = AgentIdentifier::new("aauth:assistant@.agent.example").unwrap_err();
    assert!(matches!(err, AgentIdentifierError::InvalidDomain(_)));
}

#[test]
fn rejects_domain_with_trailing_dot() {
    let err = AgentIdentifier::new("aauth:assistant@agent.example.").unwrap_err();
    assert!(matches!(err, AgentIdentifierError::InvalidDomain(_)));
}

#[test]
fn rejects_domain_with_whitespace() {
    let err = AgentIdentifier::new("aauth:assistant@agent .example").unwrap_err();
    assert!(matches!(err, AgentIdentifierError::InvalidDomain(_)));
}

#[test]
fn provider_issuer_rejects_empty() {
    let err = ProviderIssuer::new("   ").unwrap_err();
    assert!(matches!(err, ProviderIssuerError::Empty));
}

#[test]
fn provider_issuer_trims_and_accepts() {
    let iss = ProviderIssuer::new("  https://ap.example  ").expect("valid iss");
    assert_eq!(iss.as_str(), "https://ap.example");
}

#[test]
fn key_id_rejects_empty() {
    let err = KeyId::new("").unwrap_err();
    assert!(matches!(err, KeyIdError::Empty));
}

#[test]
fn token_ttl_rejects_zero() {
    let err = TokenTtl::new(0).unwrap_err();
    assert!(matches!(err, TokenTtlError::NotPositive(0)));
}

#[test]
fn token_ttl_rejects_negative() {
    let err = TokenTtl::new(-1).unwrap_err();
    assert!(matches!(err, TokenTtlError::NotPositive(-1)));
}

#[test]
fn token_ttl_accepts_positive() {
    let ttl = TokenTtl::new(3600).expect("positive ttl");
    assert_eq!(ttl.as_secs(), 3600);
}

#[test]
fn person_server_url_rejects_empty() {
    let err = PersonServerUrl::new("").unwrap_err();
    assert!(matches!(err, PersonServerUrlError::Empty));
}

fn test_encoding_key() -> EncodingKey {
    let pem = b"-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgevZzL1gdAFr88hb2\nOF/2NxApJCzGCEDdfSp6VQO30hyhRANCAAQRWz+jn65BtOMvdyHKcvjBeBSDZH2r\n1RTwjmYSi9R/zpBnuQ4EiMnCqfMPWiZqB4QdbAd0E7oH50VpuZ1P087G\n-----END PRIVATE KEY-----\n";
    EncodingKey::from_ec_pem(pem).expect("test signing key")
}

fn test_agent_jwk() -> serde_json::Value {
    serde_json::json!({
        "kty": "EC",
        "crv": "P-256",
        "x": "EVs_o5-uQbTjL3chynL4wXgUg2R9q9UU8I5mEovUf84",
        "y": "kGe5DgSIycKp8w9aJmoHhB1sB3QTugfnRWm5nU_TzsY",
    })
}

#[test]
fn mint_produces_header_and_claims_per_spec() {
    let key = AgentProviderKey::new(
        test_encoding_key(),
        KeyId::new("ap-key-1").expect("kid"),
        ProviderIssuer::new("https://ap.example").expect("iss"),
    );
    let provider = AgentProvider::new(key);
    let sub = AgentIdentifier::new("aauth:assistant-v2@agent.example").expect("sub");
    let req = AgentTokenRequest {
        sub: sub.clone(),
        agent_jwk: test_agent_jwk(),
        ttl: TokenTtl::new(3600).expect("ttl"),
        ps: Some(PersonServerUrl::new("https://ps.example").expect("ps")),
    };

    let jwt = provider.mint(req).expect("mint succeeds");

    let header = jsonwebtoken::decode_header(&jwt).expect("decode header");
    assert_eq!(header.typ.as_deref(), Some(TYP_AGENT));
    assert_eq!(header.kid.as_deref(), Some("ap-key-1"));
    assert_eq!(header.alg, Algorithm::ES256);

    let mut parts = jwt.splitn(3, '.');
    let _ = parts.next();
    let payload_b64 = parts.next().expect("payload segment");
    let payload = base64_decode(payload_b64);
    let claims: AgentClaims = serde_json::from_slice(&payload).expect("claims parse");

    assert_eq!(claims.iss, "https://ap.example");
    assert_eq!(claims.sub, sub.as_str());
    assert_eq!(claims.dwk, DWK_AGENT);
    assert_eq!(claims.cnf.jwk, test_agent_jwk());
    assert_eq!(claims.ps.as_deref(), Some("https://ps.example"));
    assert!(claims.exp > claims.iat);
    assert_eq!(claims.exp - claims.iat, 3600);
    assert!(!claims.jti.is_empty());
}

#[test]
fn mint_omits_ps_when_none() {
    let key = AgentProviderKey::new(
        test_encoding_key(),
        KeyId::new("ap-key-1").expect("kid"),
        ProviderIssuer::new("https://ap.example").expect("iss"),
    );
    let provider = AgentProvider::new(key);
    let req = AgentTokenRequest {
        sub: AgentIdentifier::new("aauth:assistant-v2@agent.example").expect("sub"),
        agent_jwk: test_agent_jwk(),
        ttl: TokenTtl::new(60).expect("ttl"),
        ps: None,
    };

    let jwt = provider.mint(req).expect("mint succeeds");
    let mut parts = jwt.splitn(3, '.');
    let _ = parts.next();
    let payload_b64 = parts.next().expect("payload segment");
    let payload = base64_decode(payload_b64);
    let claims: AgentClaims = serde_json::from_slice(&payload).expect("claims parse");
    assert_eq!(claims.ps, None);
}

#[test]
fn mint_produces_unique_jti_per_call() {
    let key = AgentProviderKey::new(
        test_encoding_key(),
        KeyId::new("ap-key-1").expect("kid"),
        ProviderIssuer::new("https://ap.example").expect("iss"),
    );
    let provider = AgentProvider::new(key);
    let make_req = || AgentTokenRequest {
        sub: AgentIdentifier::new("aauth:assistant-v2@agent.example").expect("sub"),
        agent_jwk: test_agent_jwk(),
        ttl: TokenTtl::new(60).expect("ttl"),
        ps: None,
    };

    let jwt_a = provider.mint(make_req()).expect("mint a");
    let jwt_b = provider.mint(make_req()).expect("mint b");
    assert_ne!(jwt_a, jwt_b);
}

fn base64_decode(segment: &str) -> Vec<u8> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(segment)
        .expect("valid base64url")
}
