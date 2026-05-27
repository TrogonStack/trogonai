use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use trogon_mcp_gateway::act_chain::ActChainEntry;
use trogon_mcp_gateway::jwt::{
    AgentIdentityMode, JwtIngressConfig, JwtMode, JwtValidator, VerifiedJwtClaims, check_agent_identity_violations,
    parse_verified_claims, strip_untrusted_minted_identity_claims,
};
use trogon_mcp_gateway::rpc_codes;

static HS_ISSUER: &str = "https://issuer.test/";
static UNTRUSTED_ISSUER: &str = "https://untrusted.client/";
static HS_AUD: &str = "trogon-mcp-gateway";
static HS_SECRET: LazyLock<Vec<u8>> = LazyLock::new(|| b"super-secret-demo-key-change-me".to_vec());

#[allow(clippy::cast_possible_truncation)]
fn exp_unix_secs_ahead(seconds: i64) -> serde_json::Value {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    serde_json::Value::Number((now + seconds).into())
}

fn cfg_hs256(agent_identity_mode: AgentIdentityMode) -> JwtIngressConfig {
    cfg_hs256_with_trusted(agent_identity_mode, &[HS_ISSUER])
}

fn cfg_hs256_with_trusted(agent_identity_mode: AgentIdentityMode, trusted_mint: &[&str]) -> JwtIngressConfig {
    let mut iss = HashSet::new();
    iss.insert(HS_ISSUER.into());
    iss.insert(UNTRUSTED_ISSUER.into());
    let trusted_mint_issuers: HashSet<String> = trusted_mint.iter().map(|s| (*s).to_string()).collect();
    JwtIngressConfig {
        mode: JwtMode::Require,
        agent_identity_mode,
        issuers: iss,
        trusted_mint_issuers,
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

fn base_claims(issuer: &str) -> serde_json::Value {
    serde_json::json!({
        "sub": "principal-123",
        "iss": issuer,
        "aud": HS_AUD,
        "tenant": "acme",
        "exp": exp_unix_secs_ahead(3_600),
    })
}

#[test]
fn absent_claims_parse_to_none() {
    let claims = base_claims(HS_ISSUER);
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
        "auth_method": "svid",
        "purpose": "incident-response",
        "session_id": "sess-abc",
    });

    let parsed = parse_verified_claims(&claims);
    assert_eq!(parsed.agent_id.as_deref(), Some("oncall-agent"));
    assert_eq!(parsed.auth_method.as_deref(), Some("svid"));
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
        .resolve_with_claims(Some(token.as_str()), None, true, None)
        .await
        .expect("resolve ok");

    assert_eq!(resolution.claims.agent_id.as_deref(), Some("oncall-agent"));
}

#[test]
fn off_mode_does_not_require_agent_identity() {
    check_agent_identity_violations(AgentIdentityMode::Off, "tools/call", &VerifiedJwtClaims::default());
}

#[test]
fn shadow_mode_exercises_tools_call_missing_claims_path() {
    check_agent_identity_violations(AgentIdentityMode::Shadow, "tools/call", &VerifiedJwtClaims::default());
}

#[tokio::test]
async fn enforce_rejects_missing_wkl_for_svid() {
    let validator = JwtValidator::try_new(cfg_hs256(AgentIdentityMode::Enforce)).expect("jwt cfg");
    let claims = serde_json::json!({
        "sub": "principal-123",
        "iss": HS_ISSUER,
        "aud": HS_AUD,
        "tenant": "acme",
        "exp": exp_unix_secs_ahead(3_600),
        "auth_method": "svid",
    });
    let token = mint_token(claims);

    let err = validator
        .resolve_with_claims(Some(token.as_str()), None, true, Some("tools/call"))
        .await
        .expect_err("deny");
    assert_eq!(err.code, rpc_codes::AGENT_IDENTITY_REQUIRED);
}

#[tokio::test]
async fn enforce_rejects_spiffe_wkl_without_agent_id() {
    let validator = JwtValidator::try_new(cfg_hs256(AgentIdentityMode::Enforce)).expect("jwt cfg");
    let claims = serde_json::json!({
        "sub": "principal-123",
        "iss": HS_ISSUER,
        "aud": HS_AUD,
        "tenant": "acme",
        "exp": exp_unix_secs_ahead(3_600),
        "wkl": "spiffe://acme.local/ns/prod/sa/oncall-agent",
        "auth_method": "svid",
    });
    let token = mint_token(claims);

    let err = validator
        .resolve_with_claims(Some(token.as_str()), None, true, None)
        .await
        .expect_err("deny");
    assert_eq!(err.code, rpc_codes::AGENT_IDENTITY_REQUIRED);
}

#[tokio::test]
async fn enforce_rejects_audience_mismatch_with_32109() {
    let validator = JwtValidator::try_new(cfg_hs256(AgentIdentityMode::Enforce)).expect("jwt cfg");
    let claims = serde_json::json!({
        "sub": "principal-123",
        "iss": HS_ISSUER,
        "aud": "wrong-audience",
        "tenant": "acme",
        "exp": exp_unix_secs_ahead(3_600),
    });
    let token = mint_token(claims);

    let err = validator
        .resolve_with_claims(Some(token.as_str()), None, true, None)
        .await
        .expect_err("deny");
    assert_eq!(err.code, rpc_codes::AUDIENCE_MISMATCH);
}

#[tokio::test]
async fn enforce_rejects_act_chain_depth_nine() {
    let validator = JwtValidator::try_new(cfg_hs256(AgentIdentityMode::Enforce)).expect("jwt cfg");
    let chain: Vec<ActChainEntry> = (0..9)
        .map(|i| ActChainEntry {
            sub: format!("hop-{i}"),
            agent_id: None,
            wkl: None,
            iat: i as i64,
        })
        .collect();
    let mut claims = base_claims(HS_ISSUER);
    claims
        .as_object_mut()
        .expect("object")
        .insert("act_chain".into(), serde_json::to_value(chain).unwrap());
    let token = mint_token(claims);

    let err = validator
        .resolve_with_claims(Some(token.as_str()), None, true, None)
        .await
        .expect_err("deny");
    assert_eq!(err.code, rpc_codes::ACT_CHAIN_DEPTH_EXCEEDED);
}

#[tokio::test]
async fn enforce_rejects_act_chain_agent_wkl_loop() {
    let validator = JwtValidator::try_new(cfg_hs256(AgentIdentityMode::Enforce)).expect("jwt cfg");
    let chain = vec![
        ActChainEntry {
            sub: "user-a".into(),
            agent_id: Some("acme/agent-a".into()),
            wkl: Some("spiffe://acme.local/ns/prod/sa/agent-a".into()),
            iat: 1,
        },
        ActChainEntry {
            sub: "user-b".into(),
            agent_id: Some("acme/agent-a".into()),
            wkl: Some("spiffe://acme.local/ns/prod/sa/agent-a".into()),
            iat: 2,
        },
    ];
    let mut claims = base_claims(HS_ISSUER);
    claims
        .as_object_mut()
        .expect("object")
        .insert("act_chain".into(), serde_json::to_value(chain).unwrap());
    let token = mint_token(claims);

    let err = validator
        .resolve_with_claims(Some(token.as_str()), None, true, None)
        .await
        .expect_err("deny");
    assert_eq!(err.code, rpc_codes::ACT_CHAIN_LOOP_DETECTED);
}

#[tokio::test]
async fn untrusted_issuer_strips_agent_identity_claims() {
    let validator =
        JwtValidator::try_new(cfg_hs256_with_trusted(AgentIdentityMode::Enforce, &[HS_ISSUER])).expect("jwt cfg");
    let claims = serde_json::json!({
        "sub": "principal-123",
        "iss": UNTRUSTED_ISSUER,
        "aud": HS_AUD,
        "tenant": "acme",
        "exp": exp_unix_secs_ahead(3_600),
        "agent_id": "forged-agent",
        "wkl": "spiffe://evil.local/sa/forged",
        "auth_method": "svid",
    });
    let token = mint_token(claims);

    let resolution = validator
        .resolve_with_claims(Some(token.as_str()), None, true, None)
        .await
        .expect("resolve ok");
    assert!(resolution.claims.agent_id.is_none());
    assert!(resolution.claims.wkl.is_none());
    assert!(resolution.claims.auth_method.is_none());
}

#[test]
fn strip_untrusted_minted_identity_claims_clears_fields() {
    let mut issuers = HashSet::new();
    issuers.insert(HS_ISSUER.into());
    let mut claims = VerifiedJwtClaims {
        agent_id: Some("a".into()),
        wkl: Some("spiffe://x".into()),
        wkl_attested_at: Some(1),
        auth_method: Some("svid".into()),
        act_chain: Some(vec![ActChainEntry {
            sub: "u".into(),
            agent_id: None,
            wkl: None,
            iat: 0,
        }]),
        ..VerifiedJwtClaims::default()
    };
    strip_untrusted_minted_identity_claims(Some(UNTRUSTED_ISSUER), &issuers, &mut claims);
    assert_eq!(claims, VerifiedJwtClaims::default());
}
