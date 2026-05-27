use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use trogon_mcp_gateway::jwt::{
    AgentIdentityMode, JwtIngressConfig, JwtMode, JwtValidator, VerifiedJwtClaims,
    check_agent_identity_violations, parse_verified_claims,
};

static HS_ISSUER: &str = "https://issuer.test/";
static HS_AUD: &str = "trogon-mcp-gateway";
static HS_SECRET: LazyLock<Vec<u8>> =
    LazyLock::new(|| b"super-secret-demo-key-change-me".to_vec());

#[allow(clippy::cast_possible_truncation)]
fn exp_unix_secs_ahead(seconds: i64) -> serde_json::Value {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    serde_json::Value::Number((now + seconds).into())
}

fn cfg_hs256(agent_identity_mode: AgentIdentityMode) -> JwtIngressConfig {
    let mut iss = HashSet::new();
    iss.insert(HS_ISSUER.into());
    JwtIngressConfig {
        mode: JwtMode::Validate,
        agent_identity_mode,
        issuers: iss,
        audience: HS_AUD.into(),
        leeway_secs: 60,
        tenant_claim_key: "tenant".into(),
        bearer_header_name: "authorization".into(),
        hs256_secret: Some(HS_SECRET.clone()),
        rsa_public_key_pem: None,
        jwks_uri: None,
    }
}

fn mint_token(claims: serde_json::Value) -> String {
    let enc = EncodingKey::from_secret(&HS_SECRET);
    let mut hdr = Header::new(Algorithm::HS256);
    hdr.typ = Some("JWT".into());
    encode(&hdr, &claims, &enc).expect("jwt encode")
}

#[test]
fn absent_claims_parse_to_none() {
    let claims = serde_json::json!({
        "sub": "principal-123",
        "iss": HS_ISSUER,
        "aud": HS_AUD,
        "tenant": "acme",
        "exp": exp_unix_secs_ahead(3_600),
    });

    let parsed = parse_verified_claims(&claims);
    assert_eq!(parsed, VerifiedJwtClaims::default());
}

#[test]
fn present_claims_parse_to_values() {
    let claims = serde_json::json!({
        "sub": "principal-123",
        "iss": HS_ISSUER,
        "aud": HS_AUD,
        "tenant": "acme",
        "exp": exp_unix_secs_ahead(3_600),
        "agent_id": "oncall-agent",
        "agent_version": "1.2.3",
        "wkl": "spiffe://acme.local/ns/prod/sa/oncall-agent",
        "wkl_attested_at": 1_700_000_000_i64,
        "purpose": "incident-response",
        "session_id": "sess-abc",
    });

    let parsed = parse_verified_claims(&claims);
    assert_eq!(parsed.agent_id.as_deref(), Some("oncall-agent"));
    assert_eq!(parsed.agent_version.as_deref(), Some("1.2.3"));
    assert_eq!(
        parsed.wkl.as_deref(),
        Some("spiffe://acme.local/ns/prod/sa/oncall-agent")
    );
    assert_eq!(parsed.wkl_attested_at, Some(1_700_000_000));
    assert_eq!(parsed.purpose.as_deref(), Some("incident-response"));
    assert_eq!(parsed.session_id.as_deref(), Some("sess-abc"));
}

#[tokio::test]
async fn resolve_with_claims_populates_optional_fields() {
    let validator = JwtValidator::try_new(cfg_hs256(AgentIdentityMode::Off)).expect("jwt cfg");
    let claims = serde_json::json!({
        "sub": "principal-123",
        "iss": HS_ISSUER,
        "aud": HS_AUD,
        "tenant": "acme",
        "exp": exp_unix_secs_ahead(3_600),
        "agent_id": "oncall-agent",
        "wkl": "spiffe://acme.local/ns/prod/sa/oncall-agent",
    });
    let token = mint_token(claims);

    let resolution = validator
        .resolve_with_claims(Some(token.as_str()), None, false, None)
        .await
        .expect("resolve ok");

    assert_eq!(resolution.claims.agent_id.as_deref(), Some("oncall-agent"));
    assert_eq!(
        resolution.claims.wkl.as_deref(),
        Some("spiffe://acme.local/ns/prod/sa/oncall-agent")
    );
}

#[test]
fn off_mode_does_not_require_agent_identity() {
    check_agent_identity_violations(
        AgentIdentityMode::Off,
        "tools/call",
        &VerifiedJwtClaims::default(),
    );
}

#[test]
fn shadow_mode_exercises_tools_call_missing_claims_path() {
    check_agent_identity_violations(
        AgentIdentityMode::Shadow,
        "tools/call",
        &VerifiedJwtClaims::default(),
    );
}

#[tokio::test]
async fn shadow_mode_warns_on_tools_call_with_missing_agent_id() {
    let validator = JwtValidator::try_new(cfg_hs256(AgentIdentityMode::Shadow)).expect("jwt cfg");
    let claims = serde_json::json!({
        "sub": "principal-123",
        "iss": HS_ISSUER,
        "aud": HS_AUD,
        "tenant": "acme",
        "exp": exp_unix_secs_ahead(3_600),
    });
    let token = mint_token(claims);

    validator
        .resolve_with_claims(Some(token.as_str()), None, false, Some("tools/call"))
        .await
        .expect("resolve ok");
}

#[test]
fn shadow_mode_skips_non_tools_call_methods() {
    check_agent_identity_violations(
        AgentIdentityMode::Shadow,
        "tools/list",
        &VerifiedJwtClaims::default(),
    );
}

#[test]
fn enforce_mode_exercises_log_only_stub() {
    check_agent_identity_violations(
        AgentIdentityMode::Enforce,
        "tools/call",
        &VerifiedJwtClaims::default(),
    );
}
